//! Client API handlers for memory and channel files.
//!
//! A client has the rights of `MemoryActor::User`: it reads and writes every
//! memory scope.

use std::io::ErrorKind;

use consort_proto::client_api::*;
use consort_proto::frame::Empty;
use consort_proto::model::{
    ChannelFile, ExpectedHash, MemoryAuthor, MemoryNote, MemoryScope, validate_memory_path,
    validate_relative_path,
};
use consort_proto::tools::{FilesListResult, MemoryListResult};

use consort_proto::ids::ChannelId;
use consort_proto::model::{Author, MessageKind};
use consort_proto::turn_input::SuggestionSource;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::store::conversation::NewMessage;
use crate::store::{Tx, conversation, knowledge};

/// The most bytes in the content of a memory note.
pub(crate) const MEMORY_NOTE_MAX_BYTES: usize = 256 * 1024;

/// The media type of a file whose type the client does not give.
const DEFAULT_MIME: &str = "application/octet-stream";

impl Hub {
    /// Checks and writes a memory note. A client and a coordinator both write
    /// notes through this function, so the checks are the same. The caller
    /// decides who may write the scope. This function checks the path, the
    /// size, and that a channel scope names a channel.
    ///
    /// A write by a coordinator session also posts the memory saved line in
    /// that coordinator's channel (`saved_line`), in the same transaction.
    pub(crate) async fn write_memory_note(
        &self,
        scope: MemoryScope,
        path: String,
        content: String,
        expected: ExpectedHash,
        source: WriteSource,
    ) -> Result<MemoryNote> {
        validate_memory_path(&path)?;
        if content.len() > MEMORY_NOTE_MAX_BYTES {
            return Err(HubError::invalid(format!(
                "The note is larger than {} KiB. Split it into smaller notes.",
                MEMORY_NOTE_MAX_BYTES / 1024
            )));
        }
        self.store
            .write(move |tx| {
                if let MemoryScope::Channel(channel_id) = scope {
                    let channel = conversation::channel_or_404(tx.db(), channel_id)?;
                    // The memory of the Tutti channel is the global scope.
                    if channel.is_tutti() {
                        return Err(HubError::invalid(
                            "The Tutti channel keeps its memory in the global scope. Use `global`.",
                        ));
                    }
                }
                let before = knowledge::memory_note(tx.db(), scope, &path)?;
                let note = tx.write_memory(
                    scope,
                    &path,
                    &content,
                    &expected,
                    source.by,
                    source.suggested_by,
                )?;
                let changed = before.as_ref().is_none_or(|b| b.version != note.version);
                if changed && let Some(channel_id) = source.saved_line {
                    post_memory_saved(tx, channel_id, &note, before.is_none())?;
                }
                Ok(note)
            })
            .await
    }
}

/// Who writes a memory note, and where the hub shows the write.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WriteSource {
    pub by: MemoryAuthor,
    /// The memory suggestion that the write keeps.
    pub suggested_by: Option<SuggestionSource>,
    /// The channel whose timeline gets the memory saved line. Set for a
    /// coordinator's write.
    pub saved_line: Option<ChannelId>,
}

impl WriteSource {
    /// A write by the user, from a client. It gets no line.
    pub(crate) fn user() -> Self {
        Self {
            by: MemoryAuthor::User,
            suggested_by: None,
            saved_line: None,
        }
    }
}

/// The notice kind of the memory saved line.
pub(crate) const MEMORY_SAVED: &str = "memory_saved";

/// The name of the task or channel that made a memory suggestion: the task
/// title or the channel name. Empty when it is gone.
fn suggestion_name(conn: &rusqlite::Connection, source: SuggestionSource) -> Result<String> {
    Ok(match source {
        SuggestionSource::Task(id) => crate::store::work::task(conn, id)?
            .map(|t| t.title)
            .unwrap_or_default(),
        SuggestionSource::Channel(id) => conversation::channel(conn, id)?
            .map(|c| c.name)
            .unwrap_or_default(),
    })
}

/// Posts the memory saved line in a channel's timeline: a notice with the
/// note, its version and the suggestion that it keeps. When the newest
/// timeline message is the line for the same note, the hub updates that
/// line, so a turn that writes a note twice shows one line.
fn post_memory_saved(
    tx: &mut Tx<'_>,
    channel_id: ChannelId,
    note: &MemoryNote,
    created: bool,
) -> Result<()> {
    let session = match note.updated_by {
        MemoryAuthor::Session(id) => Some(id),
        MemoryAuthor::User => None,
    };
    let newest = conversation::newest_timeline_message(tx.db(), channel_id)?;
    let same_line = newest.filter(|m| {
        m.kind == MessageKind::Notice
            && m.author_session_id == session
            && serde_json::from_str::<serde_json::Value>(&m.body).is_ok_and(|body| {
                body["kind"] == MEMORY_SAVED
                    && body["scope"] == note.scope.to_string()
                    && body["path"] == note.path
            })
    });
    // A line that said "saved the new note" keeps saying it.
    let created = created
        || same_line.as_ref().is_some_and(|m| {
            serde_json::from_str::<serde_json::Value>(&m.body)
                .is_ok_and(|body| body["created"] == true)
        });
    let verb = if created {
        "Saved the new memory note"
    } else {
        "Updated the memory note"
    };
    let name = match note.suggested_by {
        Some(source) => suggestion_name(tx.db(), source)?,
        None => String::new(),
    };
    let text = if name.is_empty() {
        format!("{verb} {}", note.path)
    } else {
        format!("{verb} {}, from a suggestion by {name}", note.path)
    };
    let body = serde_json::json!({
        "kind": MEMORY_SAVED,
        "text": text,
        "scope": note.scope,
        "path": note.path,
        "version": note.version,
        "created": created,
        "suggested_by": note.suggested_by,
        "suggested_by_name": name,
    })
    .to_string();
    match same_line {
        Some(mut line) => {
            line.body = body;
            tx.save_message(&line)
        }
        None => {
            let mut new = NewMessage::new(channel_id, Author::System, MessageKind::Notice, body);
            new.author_session_id = session;
            tx.insert_message(new).map(|_| ())
        }
    }
}

// Memory -------------------------------------------------------------------

impl Hub {
    pub(crate) async fn memory_list(
        &self,
        _caller: &Caller,
        request: MemoryList,
    ) -> Result<MemoryListResult> {
        let notes = self
            .store
            .read(move |conn| {
                if request.deleted {
                    knowledge::deleted_memory_list(conn, request.scope)
                } else {
                    knowledge::memory_list(conn, request.scope)
                }
            })
            .await?;
        Ok(MemoryListResult { notes })
    }

    pub(crate) async fn memory_read(
        &self,
        _caller: &Caller,
        request: MemoryRead,
    ) -> Result<MemoryNoteResult> {
        let note = self
            .store
            .read(move |conn| {
                knowledge::memory_note(conn, request.scope, &request.path)?.ok_or_else(|| {
                    HubError::not_found(format!(
                        "There is no memory note at {} in the {} scope.",
                        request.path, request.scope
                    ))
                })
            })
            .await?;
        Ok(MemoryNoteResult { note })
    }

    pub(crate) async fn memory_write(
        &self,
        _caller: &Caller,
        request: MemoryWrite,
    ) -> Result<MemoryNoteResult> {
        let note = self
            .write_memory_note(
                request.scope,
                request.path,
                request.content,
                request.expected_sha256,
                WriteSource::user(),
            )
            .await?;
        Ok(MemoryNoteResult { note })
    }

    pub(crate) async fn memory_restore(
        &self,
        _caller: &Caller,
        request: MemoryRestore,
    ) -> Result<MemoryNoteResult> {
        let note = self
            .store
            .write(move |tx| tx.restore_memory(request.scope, &request.path))
            .await?;
        Ok(MemoryNoteResult { note })
    }

    pub(crate) async fn memory_move(
        &self,
        _caller: &Caller,
        request: MemoryMove,
    ) -> Result<MemoryNoteResult> {
        let to = request.to_path.trim().to_owned();
        validate_memory_path(&to)?;
        let note = self
            .store
            .write(move |tx| {
                tx.move_memory(
                    request.scope,
                    &request.from_path,
                    &to,
                    &request.expected_sha256,
                )
            })
            .await?;
        Ok(MemoryNoteResult { note })
    }

    pub(crate) async fn memory_delete(
        &self,
        _caller: &Caller,
        request: MemoryDelete,
    ) -> Result<Empty> {
        self.store
            .write(move |tx| {
                tx.delete_memory(request.scope, &request.path, &request.expected_sha256)
            })
            .await?;
        Ok(Empty {})
    }

    pub(crate) async fn memory_history(
        &self,
        _caller: &Caller,
        request: MemoryHistory,
    ) -> Result<MemoryHistoryResult> {
        let mut versions = self
            .store
            .read(move |conn| knowledge::memory_history(conn, request.scope, &request.path))
            .await?;
        // The store lists the oldest version first. The client wants the
        // newest first.
        versions.reverse();
        Ok(MemoryHistoryResult { versions })
    }
}

// Channel files ------------------------------------------------------------

impl Hub {
    pub(crate) async fn files_list(
        &self,
        _caller: &Caller,
        request: FilesList,
    ) -> Result<FilesListResult> {
        let files = self
            .store
            .read(move |conn| {
                conversation::channel_or_404(conn, request.channel_id)?;
                knowledge::channel_files(conn, request.channel_id, request.prefix.as_deref())
            })
            .await?;
        Ok(FilesListResult { files })
    }

    pub(crate) async fn files_put(
        &self,
        _caller: &Caller,
        request: FilesPut,
    ) -> Result<ChannelFileResult> {
        validate_relative_path("path", &request.path)?;
        let blob_path = self.store.blobs_dir().join(request.blob_sha256.as_str());
        let blob_size = match tokio::fs::metadata(&blob_path).await {
            Ok(meta) if meta.is_file() => Some(meta.len()),
            Ok(_) => None,
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        match blob_size {
            None => {
                return Err(HubError::invalid(format!(
                    "The blob {} is not on the hub. Upload the blob first.",
                    request.blob_sha256
                )));
            }
            Some(size) if size != request.size => {
                return Err(HubError::invalid(format!(
                    "The blob has {size} bytes, but the request says {}. Send the right size, or upload the blob first.",
                    request.size
                )));
            }
            Some(_) => {}
        }
        let mime = match request.mime.trim() {
            "" => DEFAULT_MIME.to_owned(),
            mime => mime.to_owned(),
        };
        let file = self
            .store
            .write(move |tx| {
                conversation::channel_or_404(tx.db(), request.channel_id)?;
                let file = ChannelFile {
                    channel_id: request.channel_id,
                    path: request.path,
                    blob_sha256: request.blob_sha256,
                    size: request.size,
                    mime,
                    task_id: None,
                    title: None,
                    updated_by: "user".to_owned(),
                    updated_at: tx.now(),
                    deleted_at: None,
                };
                tx.save_channel_file(&file)?;
                Ok(file)
            })
            .await?;
        Ok(ChannelFileResult { file })
    }

    pub(crate) async fn files_move(
        &self,
        _caller: &Caller,
        request: FilesMove,
    ) -> Result<ChannelFileResult> {
        validate_relative_path("to_path", &request.to_path)?;
        let file = self
            .store
            .write(move |tx| {
                let mut file = knowledge::channel_file(tx.db(), request.channel_id, &request.from_path)?
                    .ok_or_else(|| {
                        HubError::not_found(format!(
                            "There is no file at {} in this channel.",
                            request.from_path
                        ))
                    })?;
                // Moving a file to its own path changes nothing.
                if request.from_path == request.to_path {
                    return Ok(file);
                }
                if knowledge::channel_file(tx.db(), request.channel_id, &request.to_path)?.is_some() {
                    return Err(HubError::conflict(format!(
                        "A file already exists at {}. Choose another path, or delete that file first.",
                        request.to_path
                    )));
                }
                tx.delete_channel_file(request.channel_id, &request.from_path)?;
                // The file keeps its blob, its task and its title.
                file.path = request.to_path;
                file.updated_by = "user".to_owned();
                file.updated_at = tx.now();
                tx.save_channel_file(&file)?;
                Ok(file)
            })
            .await?;
        Ok(ChannelFileResult { file })
    }

    pub(crate) async fn files_delete(
        &self,
        _caller: &Caller,
        request: FilesDelete,
    ) -> Result<Empty> {
        self.store
            .write(move |tx| {
                if knowledge::channel_file(tx.db(), request.channel_id, &request.path)?.is_none() {
                    return Err(HubError::not_found(format!(
                        "There is no file at {} in this channel.",
                        request.path
                    )));
                }
                tx.delete_channel_file(request.channel_id, &request.path)
            })
            .await?;
        Ok(Empty {})
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::error::ErrorCode;
    use consort_proto::ids::{ChannelId, ClientId};
    use consort_proto::model::Sha256Hash;

    use super::*;
    use crate::hub::test_hub;
    use crate::util::sha256;

    fn user() -> Caller {
        Caller::Client(ClientId::new())
    }

    fn code_of<T: std::fmt::Debug>(result: Result<T>) -> ErrorCode {
        result.unwrap_err().code()
    }

    async fn new_channel(hub: &Hub, name: &str) -> ChannelId {
        hub.channel_create(
            &Caller::Hub,
            ChannelCreate {
                name: name.to_owned(),
                purpose: String::new(),
                instructions: String::new(),
                icon: None,
                color: None,
                group_id: None,
                coordinator_selection: None,
                task_selection: None,
                repo_ids: Vec::new(),
                default_isolation: None,
                default_approval_mode: None,
                memory_enabled: None,
                tags: Vec::new(),
            },
        )
        .await
        .unwrap()
        .channel
        .id
    }

    async fn tutti_id(hub: &Hub) -> ChannelId {
        hub.store
            .read(conversation::tutti_channel)
            .await
            .unwrap()
            .id
    }

    fn write_request(
        scope: MemoryScope,
        path: &str,
        content: &str,
        expected: ExpectedHash,
    ) -> MemoryWrite {
        MemoryWrite {
            scope,
            path: path.to_owned(),
            content: content.to_owned(),
            expected_sha256: expected,
        }
    }

    /// Puts a blob in the blob folder, as the upload endpoint does.
    async fn upload(hub: &Hub, bytes: &[u8]) -> Sha256Hash {
        let hash = sha256(bytes);
        tokio::fs::write(hub.store.blobs_dir().join(hash.as_str()), bytes)
            .await
            .unwrap();
        hash
    }

    fn put_request(channel_id: ChannelId, path: &str, hash: &Sha256Hash, size: u64) -> FilesPut {
        FilesPut {
            channel_id,
            path: path.to_owned(),
            blob_sha256: hash.clone(),
            size,
            mime: "text/plain".to_owned(),
        }
    }

    #[tokio::test]
    async fn memory_write_read_list_and_history() {
        let (hub, _dir) = test_hub().await;
        let channel_id = new_channel(&hub, "Sayso").await;
        let global = MemoryScope::Global;
        let channel = MemoryScope::Channel(channel_id);

        let first = hub
            .memory_write(
                &user(),
                write_request(
                    global,
                    "people/kevin.md",
                    "Likes tea.",
                    ExpectedHash::NotExists,
                ),
            )
            .await
            .unwrap()
            .note;
        assert_eq!((first.version, first.updated_by), (1, MemoryAuthor::User));
        let second = hub
            .memory_write(
                &user(),
                write_request(
                    global,
                    "people/kevin.md",
                    "Likes coffee.",
                    ExpectedHash::Sha256(first.sha256.clone()),
                ),
            )
            .await
            .unwrap()
            .note;
        assert_eq!(second.version, 2);
        hub.memory_write(
            &user(),
            write_request(channel, "notes.md", "Use pnpm.", ExpectedHash::NotExists),
        )
        .await
        .unwrap();

        let read = hub
            .memory_read(
                &user(),
                MemoryRead {
                    scope: global,
                    path: "people/kevin.md".into(),
                },
            )
            .await
            .unwrap()
            .note;
        assert_eq!(read.content, "Likes coffee.");
        let all = hub
            .memory_list(
                &user(),
                MemoryList {
                    scope: None,
                    deleted: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(all.notes.len(), 2);
        let only_channel = hub
            .memory_list(
                &user(),
                MemoryList {
                    scope: Some(channel),
                    deleted: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(only_channel.notes.len(), 1);
        assert_eq!(only_channel.notes[0].path, "notes.md");

        let history = hub
            .memory_history(
                &user(),
                MemoryHistory {
                    scope: global,
                    path: "people/kevin.md".into(),
                },
            )
            .await
            .unwrap();
        let versions: Vec<u64> = history.versions.iter().map(|n| n.version).collect();
        assert_eq!(versions, [2, 1]);

        let missing = MemoryRead {
            scope: global,
            path: "nope.md".into(),
        };
        assert_eq!(
            code_of(hub.memory_read(&user(), missing).await),
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn memory_write_with_a_stale_hash_returns_stale_write() {
        let (hub, _dir) = test_hub().await;
        let scope = MemoryScope::Global;
        let first = hub
            .memory_write(
                &user(),
                write_request(scope, "a.md", "one", ExpectedHash::NotExists),
            )
            .await
            .unwrap()
            .note;

        // A create over an existing note is stale.
        let again = write_request(scope, "a.md", "two", ExpectedHash::NotExists);
        assert_eq!(
            code_of(hub.memory_write(&user(), again).await),
            ErrorCode::StaleWrite
        );
        // So is a write with the hash of other content.
        let wrong = write_request(scope, "a.md", "two", ExpectedHash::Sha256(sha256(b"other")));
        assert_eq!(
            code_of(hub.memory_write(&user(), wrong).await),
            ErrorCode::StaleWrite
        );
        // The note did not change.
        let read = hub
            .memory_read(
                &user(),
                MemoryRead {
                    scope,
                    path: "a.md".into(),
                },
            )
            .await
            .unwrap()
            .note;
        assert_eq!(read, first);

        // A delete with a stale hash is refused. A delete with the right hash works.
        let stale = MemoryDelete {
            scope,
            path: "a.md".into(),
            expected_sha256: sha256(b"other"),
        };
        assert_eq!(
            code_of(hub.memory_delete(&user(), stale).await),
            ErrorCode::StaleWrite
        );
        let delete = MemoryDelete {
            scope,
            path: "a.md".into(),
            expected_sha256: first.sha256.clone(),
        };
        hub.memory_delete(&user(), delete.clone()).await.unwrap();
        assert_eq!(
            code_of(hub.memory_delete(&user(), delete).await),
            ErrorCode::NotFound
        );
        // The versions of a deleted note stay.
        let history = hub
            .memory_history(
                &user(),
                MemoryHistory {
                    scope,
                    path: "a.md".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(history.versions.len(), 1);
        let none = hub
            .memory_history(
                &user(),
                MemoryHistory {
                    scope,
                    path: "never.md".into(),
                },
            )
            .await
            .unwrap();
        assert!(none.versions.is_empty());
    }

    #[tokio::test]
    async fn memory_write_checks_the_path_the_size_and_the_scope() {
        let (hub, _dir) = test_hub().await;
        let global = MemoryScope::Global;
        for path in ["notes.txt", "../x.md", "/x.md", ""] {
            let request = write_request(global, path, "x", ExpectedHash::NotExists);
            assert_eq!(
                code_of(hub.memory_write(&user(), request).await),
                ErrorCode::Invalid,
                "{path:?}"
            );
        }

        let limit = "a".repeat(MEMORY_NOTE_MAX_BYTES);
        let ok = write_request(global, "big.md", &limit, ExpectedHash::NotExists);
        assert!(hub.memory_write(&user(), ok).await.is_ok());
        let too_big = format!("{limit}a");
        let request = write_request(global, "bigger.md", &too_big, ExpectedHash::NotExists);
        assert_eq!(
            code_of(hub.memory_write(&user(), request).await),
            ErrorCode::Invalid
        );

        let unknown = MemoryScope::Channel(ChannelId::new());
        let request = write_request(unknown, "a.md", "x", ExpectedHash::NotExists);
        assert_eq!(
            code_of(hub.memory_write(&user(), request).await),
            ErrorCode::NotFound
        );
        let tutti = MemoryScope::Channel(tutti_id(&hub).await);
        let request = write_request(tutti, "a.md", "x", ExpectedHash::NotExists);
        assert_eq!(
            code_of(hub.memory_write(&user(), request).await),
            ErrorCode::Invalid
        );
    }

    #[tokio::test]
    async fn files_put_needs_an_uploaded_blob_of_the_right_size() {
        let (hub, _dir) = test_hub().await;
        let channel_id = new_channel(&hub, "Docs").await;
        let bytes = b"hello file";
        let hash = sha256(bytes);

        // The blob is not uploaded yet.
        let result = hub
            .files_put(&user(), put_request(channel_id, "notes/a.txt", &hash, 10))
            .await;
        let err = result.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Invalid);
        assert!(
            err.to_error_body()
                .message
                .contains("Upload the blob first.")
        );

        upload(&hub, bytes).await;
        let wrong_size = hub
            .files_put(&user(), put_request(channel_id, "notes/a.txt", &hash, 11))
            .await;
        assert_eq!(code_of(wrong_size), ErrorCode::Invalid);
        let bad_path = hub
            .files_put(&user(), put_request(channel_id, "../a.txt", &hash, 10))
            .await;
        assert_eq!(code_of(bad_path), ErrorCode::Invalid);
        let no_channel = hub
            .files_put(&user(), put_request(ChannelId::new(), "a.txt", &hash, 10))
            .await;
        assert_eq!(code_of(no_channel), ErrorCode::NotFound);

        let file = hub
            .files_put(&user(), put_request(channel_id, "notes/a.txt", &hash, 10))
            .await
            .unwrap()
            .file;
        assert_eq!(file.path, "notes/a.txt");
        assert_eq!(file.blob_sha256, hash);
        assert_eq!((file.size, file.updated_by.as_str()), (10, "user"));
        assert!(file.task_id.is_none() && file.title.is_none() && file.deleted_at.is_none());

        // The same path again replaces the file.
        let other = upload(&hub, b"new").await;
        let replaced = hub
            .files_put(&user(), put_request(channel_id, "notes/a.txt", &other, 3))
            .await
            .unwrap()
            .file;
        assert_eq!(replaced.blob_sha256, other);
        let listed = hub
            .files_list(
                &user(),
                FilesList {
                    channel_id,
                    prefix: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(listed.files.len(), 1);
    }

    #[tokio::test]
    async fn files_list_move_and_delete() {
        let (hub, _dir) = test_hub().await;
        let channel_id = new_channel(&hub, "Docs").await;
        let hash = upload(&hub, b"abc").await;
        for path in ["a.txt", "docs/b.txt", "docs/c.txt"] {
            hub.files_put(&user(), put_request(channel_id, path, &hash, 3))
                .await
                .unwrap();
        }
        let paths = |files: Vec<ChannelFile>| files.into_iter().map(|f| f.path).collect::<Vec<_>>();
        let prefixed = hub
            .files_list(
                &user(),
                FilesList {
                    channel_id,
                    prefix: Some("docs/".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(paths(prefixed.files), ["docs/b.txt", "docs/c.txt"]);
        let missing = FilesList {
            channel_id: ChannelId::new(),
            prefix: None,
        };
        assert_eq!(
            code_of(hub.files_list(&user(), missing).await),
            ErrorCode::NotFound
        );

        let mv = |from: &str, to: &str| FilesMove {
            channel_id,
            from_path: from.to_owned(),
            to_path: to.to_owned(),
        };
        let moved = hub
            .files_move(&user(), mv("a.txt", "archive/a.txt"))
            .await
            .unwrap()
            .file;
        assert_eq!(moved.path, "archive/a.txt");
        assert_eq!(moved.blob_sha256, hash);
        let listed = hub
            .files_list(
                &user(),
                FilesList {
                    channel_id,
                    prefix: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            paths(listed.files),
            ["archive/a.txt", "docs/b.txt", "docs/c.txt"]
        );

        // The destination must not exist. The source must exist.
        assert_eq!(
            code_of(
                hub.files_move(&user(), mv("docs/b.txt", "docs/c.txt"))
                    .await
            ),
            ErrorCode::Conflict
        );
        assert_eq!(
            code_of(hub.files_move(&user(), mv("gone.txt", "x.txt")).await),
            ErrorCode::NotFound
        );
        assert_eq!(
            code_of(hub.files_move(&user(), mv("docs/b.txt", "/abs.txt")).await),
            ErrorCode::Invalid
        );

        let delete = FilesDelete {
            channel_id,
            path: "docs/b.txt".into(),
        };
        hub.files_delete(&user(), delete.clone()).await.unwrap();
        assert_eq!(
            code_of(hub.files_delete(&user(), delete).await),
            ErrorCode::NotFound
        );
        let listed = hub
            .files_list(
                &user(),
                FilesList {
                    channel_id,
                    prefix: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(paths(listed.files), ["archive/a.txt", "docs/c.txt"]);
    }

    #[tokio::test]
    async fn moving_an_artifact_keeps_its_task_and_title() {
        let (hub, _dir) = test_hub().await;
        let channel_id = new_channel(&hub, "Docs").await;
        let hash = upload(&hub, b"report").await;
        let task_id = consort_proto::ids::TaskId::new();
        hub.store
            .write({
                let hash = hash.clone();
                move |tx| {
                    tx.save_channel_file(&ChannelFile {
                        channel_id,
                        path: "artifacts/report.pdf".to_owned(),
                        blob_sha256: hash,
                        size: 6,
                        mime: "application/pdf".to_owned(),
                        task_id: Some(task_id),
                        title: Some("Report".to_owned()),
                        updated_by: "ses".to_owned(),
                        updated_at: tx.now(),
                        deleted_at: None,
                    })
                }
            })
            .await
            .unwrap();

        let moved = hub
            .files_move(
                &user(),
                FilesMove {
                    channel_id,
                    from_path: "artifacts/report.pdf".into(),
                    to_path: "final/report.pdf".into(),
                },
            )
            .await
            .unwrap()
            .file;
        assert_eq!(moved.task_id, Some(task_id));
        assert_eq!(moved.title.as_deref(), Some("Report"));
        assert_eq!(moved.mime, "application/pdf");
        assert_eq!(moved.updated_by, "user");
    }
}
