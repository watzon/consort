//! Hub tools for messages, memory, files, search and the Tutti-only tools.
//!
//! Each method returns the JSON of the matching `*Result` type from
//! `consort_proto::tools`.

use std::collections::HashMap;
use std::path::Path;

use consort_proto::error::ErrorCode;
use consort_proto::ids::{ChannelId, MessageId};
use consort_proto::model::{
    Author, BlobRef, ChannelFile, MemoryAuthor, MemoryScope, Message, MessageKind, Sha256Hash,
    validate_relative_path,
};
use consort_proto::status::TaskStatus;
use consort_proto::tools::*;
use consort_proto::turn_input::{SuggestionSource, WakeEvent};
use rusqlite::Connection;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::api::to_value;
use crate::error::{HubError, Result};
use crate::hub::{Hub, ToolCtx};
use crate::limits::{TOOL_MESSAGES_MAX_BYTES, json_len, json_prefix};
use crate::store::conversation::{self, NewMessage};
use crate::store::{knowledge, work};
use crate::util::{hex_encode, random_bytes, sha256};

/// `fetch_timeline` page size when the call gives none, and its limit.
const TIMELINE_PAGE_DEFAULT: u32 = 50;
const TIMELINE_PAGE_MAX: u32 = 200;
/// `fetch_thread` page size when the call gives none, and its limit.
const THREAD_PAGE_DEFAULT: u32 = 50;
const THREAD_PAGE_MAX: u32 = 200;
/// The most room that a root takes in `fetch_thread` when the call names
/// one of its replies. A longer root is cut.
const THREAD_ROOT_MAX_BYTES: usize = 48 * 1024;

/// Sets the proactive mark and the suggestions of a message that a
/// coordinator just posted, and stores it again when either is set.
pub(crate) fn mark_message(
    tx: &mut crate::store::Tx<'_>,
    mut message: Message,
    posting: crate::proactivity::tools::Posting,
) -> Result<Message> {
    if posting.mark.is_none() && posting.suggestions.is_empty() {
        return Ok(message);
    }
    message.proactive = posting.mark;
    message.suggestions = posting.suggestions;
    tx.save_message(&message)?;
    Ok(message)
}

/// The bytes that a message takes in a list as JSON.
fn message_cost(message: &Message) -> Result<usize> {
    Ok(serde_json::to_vec(message)?.len() + 1)
}

/// Cuts the body of a message so that the message takes at most `room`
/// bytes as JSON. The body ends with a note that names the message ID, so
/// the coordinator can read all of it alone.
fn fit_message(mut message: Message, room: usize) -> Result<Message> {
    let cost = message_cost(&message)?;
    if cost <= room {
        return Ok(message);
    }
    let note = format!(
        "\n\n[The hub cut this message here, because the result has no room for all of it. Call fetch_thread with message_id \"{}\" to read all of it.]",
        message.id
    );
    let other = cost.saturating_sub(json_len(&message.body));
    let keep = room.saturating_sub(other + json_len(&note));
    message.body = format!("{}{note}", json_prefix(&message.body, keep));
    Ok(message)
}

/// See [`Hub::tool_fetch_thread`].
fn thread_page(
    conn: &Connection,
    channel_id: ChannelId,
    asked: Message,
    root: Message,
    before: Option<MessageId>,
    limit: usize,
) -> Result<FetchThreadResult> {
    let budget = TOOL_MESSAGES_MAX_BYTES;
    // The next page of a thread does not repeat the named reply.
    let mut message = (asked.id != root.id && before.is_none()).then_some(asked);
    let mut used = match &message {
        Some(asked) => message_cost(asked)?,
        None => 0,
    };
    let root_room = if message.is_some() {
        THREAD_ROOT_MAX_BYTES
    } else {
        budget
    };
    let root = fit_message(root, root_room.min(budget.saturating_sub(used)))?;
    used += message_cost(&root)?;
    let (rows, _) = conversation::messages_page(
        conn,
        channel_id,
        Some(root.id),
        before,
        limit + 1,
        usize::MAX,
    )?;
    let mut replies = Vec::new();
    for reply in rows.iter().take(limit) {
        if message.as_ref().is_some_and(|m| m.id == reply.id) {
            // The named reply is in the page. It is counted already.
            replies.push(reply.clone());
            message = None;
            continue;
        }
        let cost = message_cost(reply)?;
        if used + cost <= budget {
            used += cost;
            replies.push(reply.clone());
            continue;
        }
        if replies.is_empty() {
            let cut = fit_message(reply.clone(), budget.saturating_sub(used))?;
            replies.push(cut);
        }
        break;
    }
    let has_more = rows.len() > replies.len();
    replies.reverse();
    Ok(FetchThreadResult {
        root,
        replies,
        has_more,
        message,
    })
}
/// The most timeline messages that `read_channel` returns.
const READ_CHANNEL_MESSAGES: usize = 50;
/// The most hits that `search` returns.
const SEARCH_LIMIT: usize = 20;
/// The most bytes that `files_write` accepts.
const FILE_WRITE_MAX_BYTES: usize = 1024 * 1024;

fn policy_denied(message: impl Into<String>) -> HubError {
    HubError::new(ErrorCode::PolicyDenied, message)
}

/// Stops a session that is not the Tutti coordinator.
fn require_tutti(ctx: &ToolCtx) -> Result<()> {
    if ctx.audience == ToolAudience::TuttiCoordinator {
        Ok(())
    } else {
        Err(policy_denied(
            "Only the Tutti coordinator can use this tool.",
        ))
    }
}

/// The scope that a memory read uses: the one in the call, or the session's
/// own scope. The session must be allowed to read it.
fn readable_scope(ctx: &ToolCtx, requested: Option<MemoryScope>) -> Result<MemoryScope> {
    let actor = ctx.memory_actor();
    let scope = requested
        .or(actor.own_scope())
        .ok_or_else(|| HubError::invalid("Give a scope: `global` or a channel ID."))?;
    if actor.can_read(scope) {
        Ok(scope)
    } else {
        Err(policy_denied(format!(
            "You cannot read the memory in the {scope} scope. Read `global` or your own channel memory."
        )))
    }
}

/// The scope that a memory write uses: the session's own scope, if the
/// session may write it and the memory is on.
fn writable_scope(ctx: &ToolCtx) -> Result<MemoryScope> {
    let actor = ctx.memory_actor();
    let scope = actor
        .own_scope()
        .ok_or_else(|| HubError::invalid("This session has no memory scope."))?;
    if !actor.can_write(scope) {
        return Err(policy_denied(format!(
            "You cannot write memory in the {scope} scope. Use suggest_memory to propose a note."
        )));
    }
    if matches!(scope, MemoryScope::Channel(_)) && !ctx.channel.memory_enabled {
        return Err(policy_denied("Memory is off for this channel."));
    }
    Ok(scope)
}

/// Turns channel file paths into blob references.
pub(super) fn resolve_attachments(
    conn: &Connection,
    channel_id: ChannelId,
    paths: &[String],
) -> Result<Vec<BlobRef>> {
    paths
        .iter()
        .map(|path| {
            let file = knowledge::channel_file(conn, channel_id, path)?
                .filter(|f| f.deleted_at.is_none())
                .ok_or_else(|| {
                    HubError::invalid(format!(
                        "The attachment {path} is not a channel file. Use a path from files_list."
                    ))
                })?;
            Ok(BlobRef {
                sha256: file.blob_sha256,
                name: path.rsplit('/').next().unwrap_or(path).to_owned(),
                mime: file.mime,
                size: file.size,
            })
        })
        .collect()
}

fn require_text(body: &str, what: &str) -> Result<()> {
    if body.trim().is_empty() {
        Err(HubError::invalid(format!(
            "The {what} is empty. Write some text."
        )))
    } else {
        Ok(())
    }
}

/// Writes a blob into the blob folder. The write goes to a temp file in the
/// same folder first, so a reader never sees half a blob. A blob that
/// exists is left as it is.
async fn store_blob(blobs_dir: &Path, hash: &Sha256Hash, bytes: &[u8]) -> Result<()> {
    let target = blobs_dir.join(hash.as_str());
    if tokio::fs::try_exists(&target).await? {
        return Ok(());
    }
    let temp = blobs_dir.join(format!(".{hash}.{}.tmp", hex_encode(&random_bytes::<8>())));
    let written = async {
        // Only the user can read a blob. The rename keeps the mode.
        let mut file = crate::util::create_private_file(&temp).await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&temp, &target).await
    }
    .await;
    if let Err(err) = written {
        // Best effort: the temp file may not exist.
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err.into());
    }
    Ok(())
}

impl Hub {
    // Messages -------------------------------------------------------------

    pub(crate) async fn tool_post_message(
        &self,
        ctx: &ToolCtx,
        args: PostMessageArgs,
    ) -> Result<Value> {
        if args.attachments.is_empty() {
            require_text(&args.body, "message")?;
        }
        let channel_id = ctx.channel.id;
        let session_id = ctx.session.id;
        let task_id = ctx.task.as_ref().map(|t| t.id);
        let message = self
            .store
            .write(move |tx| {
                let attachments = resolve_attachments(tx.db(), channel_id, &args.attachments)?;
                let posting = crate::proactivity::tools::posting(
                    tx,
                    session_id,
                    channel_id,
                    args.notify,
                    args.suggestions,
                )?;
                let message = tx.insert_message(NewMessage {
                    author_session_id: Some(session_id),
                    author_task_id: task_id,
                    attachments,
                    ..NewMessage::new(channel_id, Author::Assistant, MessageKind::Text, args.body)
                })?;
                mark_message(tx, message, posting)
            })
            .await?;
        to_value(&MessageIdResult {
            message_id: message.id,
        })
    }

    pub(crate) async fn tool_reply(&self, ctx: &ToolCtx, args: ReplyArgs) -> Result<Value> {
        if args.attachments.is_empty() {
            require_text(&args.body, "reply")?;
        }
        let channel_id = ctx.channel.id;
        let session_id = ctx.session.id;
        let task_id = ctx.task.as_ref().map(|t| t.id);
        let message = self
            .store
            .write(move |tx| {
                let target = conversation::message_or_404(tx.db(), args.message_id)?;
                if target.channel_id != channel_id {
                    return Err(HubError::not_found(format!(
                        "The message {} is not in this channel.",
                        args.message_id
                    )));
                }
                let attachments = resolve_attachments(tx.db(), channel_id, &args.attachments)?;
                let posting = crate::proactivity::tools::posting(
                    tx,
                    session_id,
                    channel_id,
                    false,
                    args.suggestions,
                )?;
                let message = tx.insert_message(NewMessage {
                    thread_root_id: Some(args.message_id),
                    in_timeline: args.in_timeline,
                    author_session_id: Some(session_id),
                    author_task_id: task_id,
                    attachments,
                    ..NewMessage::new(channel_id, Author::Assistant, MessageKind::Text, args.body)
                })?;
                mark_message(tx, message, posting)
            })
            .await?;
        to_value(&MessageIdResult {
            message_id: message.id,
        })
    }

    pub(crate) async fn tool_update_message(
        &self,
        ctx: &ToolCtx,
        args: UpdateMessageArgs,
    ) -> Result<Value> {
        require_text(&args.body, "new text")?;
        let session_id = ctx.session.id;
        self.store
            .write(move |tx| {
                let mut message = conversation::message_or_404(tx.db(), args.message_id)?;
                if message.author != Author::Assistant
                    || message.author_session_id != Some(session_id)
                {
                    return Err(policy_denied("You can change only your own messages."));
                }
                if message.kind != MessageKind::Text || message.deleted_at.is_some() {
                    return Err(HubError::conflict(
                        "This message cannot be changed. It is deleted or it is not a text message.",
                    ));
                }
                if message.body != args.body {
                    crate::limits::check_text(
                        "message",
                        &args.body,
                        crate::limits::MESSAGE_BODY_MAX_BYTES,
                    )?;
                    message.body = args.body;
                    message.edited_at = Some(tx.now());
                    tx.save_message(&message)?;
                }
                Ok(())
            })
            .await?;
        to_value(&OkResult::OK)
    }

    pub(crate) async fn tool_react(&self, ctx: &ToolCtx, args: ReactArgs) -> Result<Value> {
        self.set_reaction(
            args.message_id,
            Some(ctx.channel.id),
            Author::Assistant,
            &args.emoji,
            false,
        )
        .await?;
        to_value(&OkResult::OK)
    }

    pub(crate) async fn tool_no_reply_needed(
        &self,
        ctx: &ToolCtx,
        args: NoReplyNeededArgs,
    ) -> Result<Value> {
        self.note_finding(ctx, args.finding).await?;
        to_value(&OkResult::OK)
    }

    pub(crate) async fn tool_fetch_timeline(
        &self,
        ctx: &ToolCtx,
        args: FetchTimelineArgs,
    ) -> Result<Value> {
        let limit = usize::try_from(
            args.limit
                .unwrap_or(TIMELINE_PAGE_DEFAULT)
                .clamp(1, TIMELINE_PAGE_MAX),
        )
        .unwrap_or(usize::MAX);
        let channel_id = ctx.channel.id;
        let (messages, has_more) = self
            .store
            .read(move |conn| {
                conversation::messages_page(
                    conn,
                    channel_id,
                    None,
                    args.before,
                    limit,
                    crate::limits::TOOL_MESSAGES_MAX_BYTES,
                )
            })
            .await?;
        to_value(&FetchTimelineResult { messages, has_more })
    }

    /// The thread of a message: the root, the message that the call names
    /// (when it is a reply), and a page of replies, all in
    /// [`TOOL_MESSAGES_MAX_BYTES`](crate::limits::TOOL_MESSAGES_MAX_BYTES).
    ///
    /// The named message comes first and is whole. The root comes next, cut
    /// when it is not the named message and it is long. Replies fill the
    /// rest, newest first. When the newest reply alone does not fit, it is
    /// cut, so a page always shows where to go on. A cut message ends with a
    /// note that names its ID.
    pub(crate) async fn tool_fetch_thread(
        &self,
        ctx: &ToolCtx,
        args: FetchThreadArgs,
    ) -> Result<Value> {
        let channel_id = ctx.channel.id;
        let limit = usize::try_from(
            args.limit
                .unwrap_or(THREAD_PAGE_DEFAULT)
                .clamp(1, THREAD_PAGE_MAX),
        )
        .unwrap_or(1);
        let result = self
            .store
            .read(move |conn| {
                let asked = conversation::message_or_404(conn, args.message_id)?;
                if asked.channel_id != channel_id {
                    return Err(HubError::not_found(format!(
                        "The message {} is not in this channel.",
                        args.message_id
                    )));
                }
                // The ID of a reply names its thread too.
                let root = match asked.thread_root_id {
                    Some(root_id) => conversation::message_or_404(conn, root_id)?,
                    None => asked.clone(),
                };
                thread_page(conn, channel_id, asked, root, args.before, limit)
            })
            .await?;
        to_value(&result)
    }

    // Memory ---------------------------------------------------------------

    pub(crate) async fn tool_memory_list(
        &self,
        ctx: &ToolCtx,
        args: MemoryListArgs,
    ) -> Result<Value> {
        let scope = readable_scope(ctx, args.scope)?;
        let notes = self
            .store
            .read(move |conn| knowledge::memory_list(conn, Some(scope)))
            .await?;
        to_value(&MemoryListResult { notes })
    }

    pub(crate) async fn tool_memory_read(
        &self,
        ctx: &ToolCtx,
        args: MemoryReadArgs,
    ) -> Result<Value> {
        let scope = readable_scope(ctx, args.scope)?;
        let note = self
            .store
            .read(move |conn| {
                knowledge::memory_note(conn, scope, &args.path)?.ok_or_else(|| {
                    HubError::not_found(format!(
                        "There is no memory note at {} in the {scope} scope.",
                        args.path
                    ))
                })
            })
            .await?;
        to_value(&MemoryReadResult {
            scope,
            path: note.path,
            content: note.content,
            sha256: note.sha256,
        })
    }

    pub(crate) async fn tool_memory_write(
        &self,
        ctx: &ToolCtx,
        args: MemoryWriteArgs,
    ) -> Result<Value> {
        let scope = writable_scope(ctx)?;
        let (session_id, channel_id, path) = (ctx.session.id, ctx.channel.id, args.path.clone());
        // The research pass never posts: its notes under `research/` get no
        // "memory saved" line.
        let research = self
            .store
            .read(move |conn| {
                crate::proactivity::tools::check_memory_write(conn, session_id, channel_id, &path)
            })
            .await?;
        let suggested_by = match args.from_suggestion {
            Some(source) => Some(source),
            None => self.only_suggestion_in_turn(ctx).await?,
        };
        let note = self
            .write_memory_note(
                scope,
                args.path,
                args.content,
                args.expected_sha256,
                crate::ops::knowledge::WriteSource {
                    by: MemoryAuthor::Session(ctx.session.id),
                    suggested_by,
                    saved_line: (!research).then_some(ctx.channel.id),
                },
            )
            .await?;
        to_value(&MemoryWriteResult {
            sha256: note.sha256,
            version: note.version,
        })
    }

    /// The source of the memory suggestion in the coordinator's open turn,
    /// when the turn holds suggestions from one source only. A write in such
    /// a turn keeps that suggestion, also when the call does not say so.
    async fn only_suggestion_in_turn(&self, ctx: &ToolCtx) -> Result<Option<SuggestionSource>> {
        let channel_id = ctx.channel.id;
        let session_id = ctx.session.id;
        let row = self
            .store
            .read(move |conn| crate::store::coordinator::coordinator(conn, channel_id))
            .await?;
        if !row.turn_open || row.session_id != Some(session_id) {
            return Ok(None);
        }
        let mut sources = row.turn_events.iter().filter_map(|event| match event {
            WakeEvent::MemorySuggestion { from, .. } => Some(*from),
            _ => None,
        });
        let first = sources.next();
        Ok(match first {
            Some(first) if sources.all(|other| other == first) => Some(first),
            _ => None,
        })
    }

    pub(crate) async fn tool_memory_delete(
        &self,
        ctx: &ToolCtx,
        args: MemoryDeleteArgs,
    ) -> Result<Value> {
        let scope = writable_scope(ctx)?;
        if crate::ops::knowledge::is_voice_note(scope, &args.path) {
            return Err(policy_denied(crate::voice::VOICE_NOTE_REFUSAL));
        }
        self.store
            .write(move |tx| tx.delete_memory(scope, &args.path, &args.expected_sha256))
            .await?;
        to_value(&OkResult::OK)
    }

    // Files ----------------------------------------------------------------

    pub(crate) async fn tool_files_list(
        &self,
        ctx: &ToolCtx,
        args: FilesListArgs,
    ) -> Result<Value> {
        let channel_id = ctx.channel.id;
        let files = self
            .store
            .read(move |conn| knowledge::channel_files(conn, channel_id, args.prefix.as_deref()))
            .await?;
        to_value(&FilesListResult { files })
    }

    pub(crate) async fn tool_files_write(
        &self,
        ctx: &ToolCtx,
        args: FilesWriteArgs,
    ) -> Result<Value> {
        validate_relative_path("path", &args.path)?;
        let bytes = args.content.as_bytes();
        if bytes.len() > FILE_WRITE_MAX_BYTES {
            return Err(HubError::invalid(format!(
                "The file is larger than {} MiB. Write a smaller text file.",
                FILE_WRITE_MAX_BYTES / (1024 * 1024)
            )));
        }
        let hash = sha256(bytes);
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        store_blob(self.store.blobs_dir(), &hash, bytes).await?;
        let mime = if args.path.to_ascii_lowercase().ends_with(".md") {
            "text/markdown"
        } else {
            "text/plain"
        };
        let channel_id = ctx.channel.id;
        let updated_by = ctx.session.id.to_string();
        let result = FilesWriteResult {
            path: args.path.clone(),
            sha256: hash.clone(),
            size,
        };
        self.store
            .write(move |tx| {
                tx.save_channel_file(&ChannelFile {
                    channel_id,
                    path: args.path,
                    blob_sha256: hash,
                    size,
                    mime: mime.to_owned(),
                    task_id: None,
                    title: None,
                    updated_by,
                    updated_at: tx.now(),
                    deleted_at: None,
                })
            })
            .await?;
        to_value(&result)
    }

    // Search ---------------------------------------------------------------

    pub(crate) async fn tool_search(&self, ctx: &ToolCtx, args: SearchArgs) -> Result<Value> {
        let sees_every_channel = ctx.audience == ToolAudience::TuttiCoordinator;
        let own_channel = ctx.channel.id;
        if !sees_every_channel && args.channel_id.is_some_and(|id| id != own_channel) {
            return Err(policy_denied(
                "You can search only this channel. Leave out channel_id, or use the ID of this channel.",
            ));
        }
        let actor = ctx.memory_actor();
        let hits = self
            .store
            .read(move |conn| {
                knowledge::search(
                    conn,
                    &args.query,
                    args.channel_id,
                    &[],
                    SEARCH_LIMIT,
                    &|hit| match hit {
                        SearchHit::Message { channel_id, .. }
                        | SearchHit::Task { channel_id, .. } => {
                            sees_every_channel || *channel_id == own_channel
                        }
                        SearchHit::MemoryNote { scope, .. } => actor.can_read(*scope),
                    },
                )
            })
            .await?;
        to_value(&SearchResult { hits })
    }

    // The Tutti coordinator -------------------------------------------

    pub(crate) async fn tool_list_channels(&self, ctx: &ToolCtx, _args: NoArgs) -> Result<Value> {
        require_tutti(ctx)?;
        let channels = self
            .store
            .read(|conn| {
                let mut counts: HashMap<ChannelId, TaskCounts> = HashMap::new();
                for task in work::tasks_not_archived(conn)? {
                    counts.entry(task.channel_id).or_default().add(task.status);
                }
                Ok(conversation::channels(conn)?
                    .into_iter()
                    .map(|channel| ChannelSummary {
                        tasks: counts.get(&channel.id).copied().unwrap_or_default(),
                        archived: channel.archived_at.is_some(),
                        id: channel.id,
                        name: channel.name,
                        kind: channel.kind,
                        purpose: channel.purpose,
                    })
                    .collect())
            })
            .await?;
        to_value(&ListChannelsResult { channels })
    }

    pub(crate) async fn tool_read_channel(
        &self,
        ctx: &ToolCtx,
        args: ReadChannelArgs,
    ) -> Result<Value> {
        require_tutti(ctx)?;
        let result = self
            .store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, args.channel_id)?;
                let (mut messages, _) = conversation::messages_page(
                    conn,
                    channel.id,
                    None,
                    None,
                    READ_CHANNEL_MESSAGES,
                    crate::limits::TOOL_MESSAGES_MAX_BYTES,
                )?;
                if let Some(since) = args.since {
                    messages.retain(|m| m.created_at > since);
                }
                // The page is newest first. The tool shows oldest first.
                messages.reverse();
                let tasks = work::tasks_in_channel(conn, channel.id, false)?
                    .into_iter()
                    .filter(|t| t.status != TaskStatus::Resolved)
                    .collect();
                let memory = knowledge::memory_list(conn, Some(channel.memory_scope()))?;
                Ok(ReadChannelResult {
                    channel,
                    messages,
                    tasks,
                    memory,
                })
            })
            .await?;
        to_value(&result)
    }

    pub(crate) async fn tool_message_channel(
        &self,
        ctx: &ToolCtx,
        args: MessageChannelArgs,
    ) -> Result<Value> {
        require_tutti(ctx)?;
        require_text(&args.body, "message")?;
        let session_id = ctx.session.id;
        let target = args.channel_id;
        let message = self
            .store
            .write(move |tx| {
                let channel = conversation::channel_or_404(tx.db(), target)?;
                if channel.is_tutti() {
                    return Err(HubError::conflict(
                        "You cannot use message_channel for the Tutti channel. Use post_message.",
                    ));
                }
                let message = tx.insert_message(NewMessage {
                    author_session_id: Some(session_id),
                    ..NewMessage::new(target, Author::Assistant, MessageKind::Text, args.body)
                })?;
                tx.queue_wake(
                    target,
                    &WakeEvent::TuttiMessage {
                        id: message.id,
                        at: message.created_at,
                        text: message.body.clone(),
                    },
                )?;
                Ok(message)
            })
            .await?;
        self.kick_coordinator(target);
        to_value(&MessageIdResult {
            message_id: message.id,
        })
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::client_api::{ChannelCreate, MessageSend};
    use consort_proto::ids::{AccountId, MessageId, SessionId, TaskId, WorkerId};
    use consort_proto::model::{
        Channel, ExpectedHash, Message, ModelOptions, Session, SessionRole, SessionState, Task,
        TaskCreator, Usage,
    };
    use consort_proto::time::Timestamp;
    use serde::de::DeserializeOwned;

    use super::*;
    use crate::hub::{Caller, test_hub};
    use crate::store::coordinator::pending_wakes;

    // Helpers ---------------------------------------------------------------

    fn parse<T: DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    fn code_of<T: std::fmt::Debug>(result: Result<T>) -> ErrorCode {
        result.unwrap_err().code()
    }

    async fn new_channel(hub: &Hub, name: &str) -> Channel {
        hub.channel_create(
            &Caller::Hub,
            ChannelCreate {
                name: name.to_owned(),
                purpose: format!("About {name}"),
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
    }

    async fn tutti(hub: &Hub) -> Channel {
        hub.store.read(conversation::tutti_channel).await.unwrap()
    }

    async fn reload(hub: &Hub, channel: &Channel) -> Channel {
        let id = channel.id;
        hub.store
            .read(move |conn| conversation::channel_or_404(conn, id))
            .await
            .unwrap()
    }

    /// A tool context for a coordinator or task session in `channel`. The
    /// session is saved in the store.
    async fn ctx_for(hub: &Hub, channel: &Channel, role: SessionRole) -> ToolCtx {
        let session = Session {
            id: SessionId::new(),
            role,
            channel_id: channel.id,
            task_id: None,
            worker_id: WorkerId::new(),
            account_id: AccountId::new(),
            harness: "fake".to_owned(),
            model: "fake-1".to_owned(),
            options: ModelOptions::new(),
            workspace_path: "/tmp/workspace".to_owned(),
            native_ref: None,
            state: SessionState::Idle,
            last_seq: 0,
            usage: Usage::default(),
            started_at: Timestamp::now(),
            ended_at: None,
            end_reason: None,
        };
        let saved = session.clone();
        hub.store
            .write(move |tx| tx.save_session(&saved))
            .await
            .unwrap();
        ToolCtx {
            audience: ToolAudience::for_session(role, channel.kind),
            session,
            channel: channel.clone(),
            task: None,
        }
    }

    async fn user_message(hub: &Hub, channel_id: ChannelId, body: &str) -> Message {
        hub.message_send(
            &Caller::Hub,
            MessageSend {
                channel_id,
                client_message_id: String::new(),
                body: body.to_owned(),
                attachments: Vec::new(),
                thread_root_id: None,
                in_timeline: false,
            },
        )
        .await
        .unwrap()
        .message
    }

    async fn message_now(hub: &Hub, id: MessageId) -> Message {
        hub.store
            .read(move |conn| conversation::message_or_404(conn, id))
            .await
            .unwrap()
    }

    async fn post(hub: &Hub, ctx: &ToolCtx, body: &str) -> MessageId {
        let result = hub
            .tool_post_message(
                ctx,
                PostMessageArgs {
                    body: body.to_owned(),
                    attachments: Vec::new(),
                    notify: false,
                    suggestions: Vec::new(),
                },
            )
            .await
            .unwrap();
        parse::<MessageIdResult>(result).message_id
    }

    async fn make_task(hub: &Hub, channel_id: ChannelId, title: &str, status: TaskStatus) -> Task {
        let title = title.to_owned();
        hub.store
            .write(move |tx| {
                let card = tx.insert_message(NewMessage::new(
                    channel_id,
                    Author::System,
                    MessageKind::TaskCard,
                    "",
                ))?;
                let now = tx.now();
                let task = Task {
                    id: TaskId::new(),
                    channel_id,
                    title,
                    brief: "Do the work".to_owned(),
                    status,
                    status_reason: None,
                    created_by: TaskCreator::User,
                    card_message_id: card.id,
                    model_selection: None,
                    placement: Default::default(),
                    approval_mode: Default::default(),
                    start_at: None,
                    current_session_id: None,
                    worker_id: None,
                    review_summary: None,
                    resolved_at: None,
                    session_ended: false,
                    archived_at: None,
                    created_at: now,
                    updated_at: now,
                };
                tx.save_task(&task)?;
                Ok(task)
            })
            .await
            .unwrap()
    }

    async fn write_note(hub: &Hub, ctx: &ToolCtx, path: &str, content: &str) -> MemoryWriteResult {
        let result = hub
            .tool_memory_write(
                ctx,
                MemoryWriteArgs {
                    path: path.to_owned(),
                    content: content.to_owned(),
                    expected_sha256: ExpectedHash::NotExists,
                    from_suggestion: None,
                },
            )
            .await
            .unwrap();
        parse(result)
    }

    // Messages --------------------------------------------------------------

    #[tokio::test]
    async fn post_message_stores_an_assistant_message_in_the_timeline() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;

        let id = post(&hub, &ctx, "Hello").await;
        let message = message_now(&hub, id).await;
        assert_eq!(message.author, Author::Assistant);
        assert_eq!(message.kind, MessageKind::Text);
        assert_eq!(message.author_session_id, Some(ctx.session.id));
        assert_eq!(message.channel_id, channel.id);
        assert_eq!(message.thread_root_id, None);
        // A coordinator message does not wake the coordinator.
        let id = channel.id;
        let wakes = hub
            .store
            .read(move |conn| pending_wakes(conn, id))
            .await
            .unwrap();
        assert!(wakes.is_empty());

        let empty = hub
            .tool_post_message(
                &ctx,
                PostMessageArgs {
                    body: " ".to_owned(),
                    attachments: Vec::new(),
                    notify: false,
                    suggestions: Vec::new(),
                },
            )
            .await;
        assert_eq!(code_of(empty), ErrorCode::Invalid);
    }

    #[tokio::test]
    async fn post_message_attaches_channel_files_by_path() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let written: FilesWriteResult = parse(
            hub.tool_files_write(
                &ctx,
                FilesWriteArgs {
                    path: "reports/summary.md".to_owned(),
                    content: "# Summary".to_owned(),
                },
            )
            .await
            .unwrap(),
        );

        // An attachment alone is a message.
        let result = hub
            .tool_post_message(
                &ctx,
                PostMessageArgs {
                    body: String::new(),
                    attachments: vec!["reports/summary.md".to_owned()],
                    notify: false,
                    suggestions: Vec::new(),
                },
            )
            .await
            .unwrap();
        let message = message_now(&hub, parse::<MessageIdResult>(result).message_id).await;
        assert_eq!(
            message.attachments,
            [BlobRef {
                sha256: written.sha256,
                name: "summary.md".to_owned(),
                mime: "text/markdown".to_owned(),
                size: 9,
            }]
        );

        let unknown = hub
            .tool_post_message(
                &ctx,
                PostMessageArgs {
                    body: "See the file".to_owned(),
                    attachments: vec!["reports/missing.md".to_owned()],
                    notify: false,
                    suggestions: Vec::new(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(unknown.code(), ErrorCode::Invalid);
        assert!(
            unknown
                .to_error_body()
                .message
                .contains("reports/missing.md")
        );
    }

    #[tokio::test]
    async fn reply_goes_in_the_thread_of_a_message_in_this_channel() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let other = new_channel(&hub, "Other").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let root = user_message(&hub, channel.id, "question").await;
        let elsewhere = user_message(&hub, other.id, "other question").await;

        let result = hub
            .tool_reply(
                &ctx,
                ReplyArgs {
                    message_id: root.id,
                    body: "answer".to_owned(),
                    in_timeline: true,
                    attachments: Vec::new(),
                    suggestions: Vec::new(),
                },
            )
            .await
            .unwrap();
        let reply = message_now(&hub, parse::<MessageIdResult>(result).message_id).await;
        assert_eq!(reply.thread_root_id, Some(root.id));
        assert!(reply.in_timeline);
        assert_eq!(reply.author, Author::Assistant);
        assert_eq!(reply.author_session_id, Some(ctx.session.id));
        assert_eq!(message_now(&hub, root.id).await.reply_count, 1);

        let wrong_channel = hub
            .tool_reply(
                &ctx,
                ReplyArgs {
                    message_id: elsewhere.id,
                    body: "x".to_owned(),
                    in_timeline: false,
                    attachments: Vec::new(),
                    suggestions: Vec::new(),
                },
            )
            .await;
        assert_eq!(code_of(wrong_channel), ErrorCode::NotFound);
        let empty = hub
            .tool_reply(
                &ctx,
                ReplyArgs {
                    message_id: root.id,
                    body: String::new(),
                    in_timeline: false,
                    attachments: Vec::new(),
                    suggestions: Vec::new(),
                },
            )
            .await;
        assert_eq!(code_of(empty), ErrorCode::Invalid);
    }

    #[tokio::test]
    async fn update_message_changes_only_the_messages_of_this_session() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let later = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let mine = post(&hub, &ctx, "draft").await;
        let users = user_message(&hub, channel.id, "hello").await;

        let args = |message_id, body: &str| UpdateMessageArgs {
            message_id,
            body: body.to_owned(),
        };
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        let result = hub
            .tool_update_message(&ctx, args(mine, "final"))
            .await
            .unwrap();
        assert_eq!(parse::<OkResult>(result), OkResult::OK);
        let updated = message_now(&hub, mine).await;
        assert_eq!(updated.body, "final");
        assert!(updated.edited_at.is_some());

        // Another session, and a message of the user, are not yours.
        let other_session = hub.tool_update_message(&later, args(mine, "hijack")).await;
        assert_eq!(code_of(other_session), ErrorCode::PolicyDenied);
        let user_message = hub
            .tool_update_message(&ctx, args(users.id, "hijack"))
            .await;
        assert_eq!(code_of(user_message), ErrorCode::PolicyDenied);
        assert_eq!(message_now(&hub, mine).await.body, "final");
        assert_eq!(message_now(&hub, users.id).await.body, "hello");
        let missing = hub
            .tool_update_message(&ctx, args(MessageId::new(), "x"))
            .await;
        assert_eq!(code_of(missing), ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn react_adds_an_assistant_reaction_and_no_reply_needed_confirms() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let other = new_channel(&hub, "Other").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let message = user_message(&hub, channel.id, "great news").await;
        let elsewhere = user_message(&hub, other.id, "x").await;

        for _ in 0..2 {
            let result = hub
                .tool_react(
                    &ctx,
                    ReactArgs {
                        message_id: message.id,
                        emoji: "🎉".to_owned(),
                    },
                )
                .await
                .unwrap();
            assert_eq!(parse::<OkResult>(result), OkResult::OK);
        }
        let reactions = message_now(&hub, message.id).await.reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].author, Author::Assistant);
        assert_eq!(reactions[0].emoji, "🎉");

        let wrong_channel = hub
            .tool_react(
                &ctx,
                ReactArgs {
                    message_id: elsewhere.id,
                    emoji: "🎉".to_owned(),
                },
            )
            .await;
        assert_eq!(code_of(wrong_channel), ErrorCode::NotFound);

        let done = hub
            .tool_no_reply_needed(&ctx, NoReplyNeededArgs::default())
            .await
            .unwrap();
        assert_eq!(done, serde_json::json!({"ok": true}));
    }

    #[tokio::test]
    async fn fetch_timeline_and_fetch_thread_stay_in_this_channel() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let other = new_channel(&hub, "Other").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let mut sent = Vec::new();
        for i in 0..4 {
            sent.push(user_message(&hub, channel.id, &format!("m{i}")).await);
        }
        let elsewhere = user_message(&hub, other.id, "elsewhere").await;
        let reply_id = parse::<MessageIdResult>(
            hub.tool_reply(
                &ctx,
                ReplyArgs {
                    message_id: sent[0].id,
                    body: "reply".to_owned(),
                    in_timeline: false,
                    attachments: Vec::new(),
                    suggestions: Vec::new(),
                },
            )
            .await
            .unwrap(),
        )
        .message_id;

        let page: FetchTimelineResult = parse(
            hub.tool_fetch_timeline(
                &ctx,
                FetchTimelineArgs {
                    before: None,
                    limit: Some(3),
                },
            )
            .await
            .unwrap(),
        );
        // Newest first. The thread reply is not in the timeline.
        let ids: Vec<_> = page.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, [sent[3].id, sent[2].id, sent[1].id]);
        assert!(page.has_more);
        let older: FetchTimelineResult = parse(
            hub.tool_fetch_timeline(
                &ctx,
                FetchTimelineArgs {
                    before: Some(sent[1].id),
                    limit: None,
                },
            )
            .await
            .unwrap(),
        );
        assert_eq!(older.messages.len(), 1);
        assert!(!older.has_more);

        let thread: FetchThreadResult = parse(
            hub.tool_fetch_thread(
                &ctx,
                FetchThreadArgs {
                    message_id: sent[0].id,
                    before: None,
                    limit: None,
                },
            )
            .await
            .unwrap(),
        );
        assert_eq!(thread.root.id, sent[0].id);
        assert_eq!(thread.replies.len(), 1);
        assert_eq!(thread.replies[0].id, reply_id);
        // The ID of a reply gives the same thread.
        let from_reply: FetchThreadResult = parse(
            hub.tool_fetch_thread(
                &ctx,
                FetchThreadArgs {
                    message_id: reply_id,
                    before: None,
                    limit: None,
                },
            )
            .await
            .unwrap(),
        );
        assert_eq!(from_reply.root.id, sent[0].id);

        let wrong_channel = hub
            .tool_fetch_thread(
                &ctx,
                FetchThreadArgs {
                    message_id: elsewhere.id,
                    before: None,
                    limit: None,
                },
            )
            .await;
        assert_eq!(code_of(wrong_channel), ErrorCode::NotFound);
    }

    // Memory ----------------------------------------------------------------

    #[tokio::test]
    async fn memory_rights_follow_the_memory_actor_rules() {
        let (hub, _dir) = test_hub().await;
        let tutti = tutti(&hub).await;
        let a = new_channel(&hub, "A").await;
        let b = new_channel(&hub, "B").await;
        let tutti_ctx = ctx_for(&hub, &tutti, SessionRole::Coordinator).await;
        let a_ctx = ctx_for(&hub, &a, SessionRole::Coordinator).await;
        let b_ctx = ctx_for(&hub, &b, SessionRole::Coordinator).await;
        let task_ctx = ctx_for(&hub, &a, SessionRole::Task).await;
        assert_eq!(task_ctx.audience, ToolAudience::TaskSession);

        // Each coordinator writes its own scope.
        write_note(&hub, &tutti_ctx, "me.md", "The user is Chris.").await;
        write_note(&hub, &a_ctx, "a.md", "Channel A notes.").await;
        write_note(&hub, &b_ctx, "b.md", "Channel B notes.").await;

        let read = |ctx: &ToolCtx, scope: Option<MemoryScope>, path: &str| {
            let args = MemoryReadArgs {
                scope,
                path: path.to_owned(),
            };
            let ctx = ctx.clone();
            let hub = hub.clone();
            async move { hub.tool_memory_read(&ctx, args).await }
        };
        let list = |ctx: &ToolCtx, scope: Option<MemoryScope>| {
            let ctx = ctx.clone();
            let hub = hub.clone();
            async move {
                hub.tool_memory_list(&ctx, MemoryListArgs { scope })
                    .await
                    .map(parse::<MemoryListResult>)
            }
        };
        let global = Some(MemoryScope::Global);
        let scope_a = Some(MemoryScope::Channel(a.id));
        let scope_b = Some(MemoryScope::Channel(b.id));

        // The Tutti coordinator reads every scope. It defaults to global.
        let default = list(&tutti_ctx, None).await.unwrap();
        assert_eq!(default.notes.len(), 1);
        assert_eq!(default.notes[0].path, "me.md");
        assert!(read(&tutti_ctx, scope_b, "b.md").await.is_ok());
        assert_eq!(list(&tutti_ctx, scope_a).await.unwrap().notes.len(), 1);

        // A channel coordinator defaults to its channel, reads global, and
        // cannot read another channel.
        let own = list(&a_ctx, None).await.unwrap();
        assert_eq!(own.notes[0].path, "a.md");
        let read_global: MemoryReadResult = parse(read(&a_ctx, global, "me.md").await.unwrap());
        assert_eq!(read_global.content, "The user is Chris.");
        assert_eq!(read_global.scope, MemoryScope::Global);
        let denied = read(&a_ctx, scope_b, "b.md").await.unwrap_err();
        assert_eq!(denied.code(), ErrorCode::PolicyDenied);
        assert!(denied.to_error_body().message.contains(&b.id.to_string()));
        assert_eq!(
            code_of(list(&a_ctx, scope_b).await),
            ErrorCode::PolicyDenied
        );
        assert_eq!(
            code_of(read(&a_ctx, None, "missing.md").await),
            ErrorCode::NotFound
        );

        // A task session reads global and its channel scope, and cannot write.
        assert!(read(&task_ctx, global, "me.md").await.is_ok());
        assert!(read(&task_ctx, None, "a.md").await.is_ok());
        assert_eq!(
            code_of(read(&task_ctx, scope_b, "b.md").await),
            ErrorCode::PolicyDenied
        );
        let write = hub
            .tool_memory_write(
                &task_ctx,
                MemoryWriteArgs {
                    path: "t.md".to_owned(),
                    content: "x".to_owned(),
                    expected_sha256: ExpectedHash::NotExists,
                    from_suggestion: None,
                },
            )
            .await;
        assert_eq!(code_of(write), ErrorCode::PolicyDenied);
        let delete = hub
            .tool_memory_delete(
                &task_ctx,
                MemoryDeleteArgs {
                    path: "a.md".to_owned(),
                    expected_sha256: sha256(b"Channel A notes."),
                },
            )
            .await;
        assert_eq!(code_of(delete), ErrorCode::PolicyDenied);

        // The writes landed in the right scopes, signed by the session.
        let note = hub
            .store
            .read({
                let id = b.id;
                move |conn| knowledge::memory_note(conn, MemoryScope::Channel(id), "b.md")
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(note.updated_by, MemoryAuthor::Session(b_ctx.session.id));
        let global_note = hub
            .store
            .read(|conn| knowledge::memory_note(conn, MemoryScope::Global, "me.md"))
            .await
            .unwrap();
        assert!(global_note.is_some());
    }

    #[tokio::test]
    async fn memory_write_returns_stale_write_and_delete_checks_the_hash() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "A").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let first = write_note(&hub, &ctx, "a.md", "one").await;
        assert_eq!(first.version, 1);

        let write = |expected: ExpectedHash, content: &str| MemoryWriteArgs {
            path: "a.md".to_owned(),
            content: content.to_owned(),
            expected_sha256: expected,
            from_suggestion: None,
        };
        let stale = hub
            .tool_memory_write(&ctx, write(ExpectedHash::Sha256(sha256(b"other")), "two"))
            .await;
        assert_eq!(code_of(stale), ErrorCode::StaleWrite);
        let create_again = hub
            .tool_memory_write(&ctx, write(ExpectedHash::NotExists, "two"))
            .await;
        assert_eq!(code_of(create_again), ErrorCode::StaleWrite);
        let second: MemoryWriteResult = parse(
            hub.tool_memory_write(
                &ctx,
                write(ExpectedHash::Sha256(first.sha256.clone()), "two"),
            )
            .await
            .unwrap(),
        );
        assert_eq!(second.version, 2);
        assert_eq!(second.sha256, sha256(b"two"));

        let bad_path = hub
            .tool_memory_write(
                &ctx,
                MemoryWriteArgs {
                    path: "a.txt".to_owned(),
                    content: "x".to_owned(),
                    expected_sha256: ExpectedHash::NotExists,
                    from_suggestion: None,
                },
            )
            .await;
        assert_eq!(code_of(bad_path), ErrorCode::Invalid);

        let stale_delete = hub
            .tool_memory_delete(
                &ctx,
                MemoryDeleteArgs {
                    path: "a.md".to_owned(),
                    expected_sha256: first.sha256,
                },
            )
            .await;
        assert_eq!(code_of(stale_delete), ErrorCode::StaleWrite);
        let deleted = hub
            .tool_memory_delete(
                &ctx,
                MemoryDeleteArgs {
                    path: "a.md".to_owned(),
                    expected_sha256: second.sha256,
                },
            )
            .await
            .unwrap();
        assert_eq!(parse::<OkResult>(deleted), OkResult::OK);
        let listed: MemoryListResult = parse(
            hub.tool_memory_list(&ctx, MemoryListArgs { scope: None })
                .await
                .unwrap(),
        );
        assert!(listed.notes.is_empty());
    }

    #[tokio::test]
    async fn memory_writes_are_refused_when_channel_memory_is_off() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "A").await;
        hub.channel_update(&Caller::Hub, {
            let mut update = consort_proto::client_api::ChannelUpdate::new(channel.id);
            update.memory_enabled = Some(false);
            update
        })
        .await
        .unwrap();
        let channel = reload(&hub, &channel).await;
        assert!(!channel.memory_enabled);
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;

        let write = hub
            .tool_memory_write(
                &ctx,
                MemoryWriteArgs {
                    path: "a.md".to_owned(),
                    content: "x".to_owned(),
                    expected_sha256: ExpectedHash::NotExists,
                    from_suggestion: None,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(write.code(), ErrorCode::PolicyDenied);
        assert_eq!(
            write.to_error_body().message,
            "Memory is off for this channel."
        );
        let delete = hub
            .tool_memory_delete(
                &ctx,
                MemoryDeleteArgs {
                    path: "a.md".to_owned(),
                    expected_sha256: sha256(b"x"),
                },
            )
            .await;
        assert_eq!(code_of(delete), ErrorCode::PolicyDenied);

        // The Tutti channel writes global memory, which has no switch.
        let tutti_ctx = ctx_for(&hub, &tutti(&hub).await, SessionRole::Coordinator).await;
        write_note(&hub, &tutti_ctx, "me.md", "x").await;
    }

    // Files -----------------------------------------------------------------

    #[tokio::test]
    async fn files_write_then_files_list() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Docs").await;
        let other = new_channel(&hub, "Other").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let write = |path: &str, content: &str| FilesWriteArgs {
            path: path.to_owned(),
            content: content.to_owned(),
        };

        let notes: FilesWriteResult = parse(
            hub.tool_files_write(&ctx, write("notes/todo.md", "- ship it"))
                .await
                .unwrap(),
        );
        assert_eq!(notes.path, "notes/todo.md");
        assert_eq!(notes.size, 9);
        assert_eq!(notes.sha256, sha256(b"- ship it"));
        let plain: FilesWriteResult = parse(
            hub.tool_files_write(&ctx, write("data.csv", "a,b"))
                .await
                .unwrap(),
        );

        // The blob is on disk, under its hash, with no temp file left over.
        let blob = tokio::fs::read(hub.store.blobs_dir().join(notes.sha256.as_str()))
            .await
            .unwrap();
        assert_eq!(blob, b"- ship it");
        let mut names = Vec::new();
        let mut dir = tokio::fs::read_dir(hub.store.blobs_dir()).await.unwrap();
        while let Some(entry) = dir.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
        assert_eq!(names.len(), 2);

        let listed: FilesListResult = parse(
            hub.tool_files_list(&ctx, FilesListArgs { prefix: None })
                .await
                .unwrap(),
        );
        let summary: Vec<_> = listed
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.mime.as_str(), f.updated_by.clone()))
            .collect();
        assert_eq!(
            summary,
            [
                ("data.csv", "text/plain", ctx.session.id.to_string()),
                ("notes/todo.md", "text/markdown", ctx.session.id.to_string()),
            ]
        );
        assert_eq!(listed.files[0].blob_sha256, plain.sha256);
        let prefixed: FilesListResult = parse(
            hub.tool_files_list(
                &ctx,
                FilesListArgs {
                    prefix: Some("notes/".to_owned()),
                },
            )
            .await
            .unwrap(),
        );
        assert_eq!(prefixed.files.len(), 1);

        // The same content again, and new content at the same path.
        hub.tool_files_write(&ctx, write("copy.txt", "a,b"))
            .await
            .unwrap();
        hub.tool_files_write(&ctx, write("data.csv", "a,b,c"))
            .await
            .unwrap();
        let listed: FilesListResult = parse(
            hub.tool_files_list(&ctx, FilesListArgs { prefix: None })
                .await
                .unwrap(),
        );
        assert_eq!(listed.files.len(), 3);

        // Other channels have their own files.
        let other_ctx = ctx_for(&hub, &other, SessionRole::Coordinator).await;
        let none: FilesListResult = parse(
            hub.tool_files_list(&other_ctx, FilesListArgs { prefix: None })
                .await
                .unwrap(),
        );
        assert!(none.files.is_empty());
    }

    #[tokio::test]
    async fn files_write_checks_the_path_and_the_size() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Docs").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let write = |path: &str, content: String| FilesWriteArgs {
            path: path.to_owned(),
            content,
        };

        for path in ["", "/abs.txt", "../up.txt", "a//b.txt", "a\\b.txt"] {
            let result = hub
                .tool_files_write(&ctx, write(path, "x".to_owned()))
                .await;
            assert_eq!(code_of(result), ErrorCode::Invalid, "{path:?}");
        }
        let limit = "a".repeat(FILE_WRITE_MAX_BYTES);
        assert!(
            hub.tool_files_write(&ctx, write("big.txt", limit.clone()))
                .await
                .is_ok()
        );
        let too_big = hub
            .tool_files_write(&ctx, write("bigger.txt", format!("{limit}a")))
            .await;
        assert_eq!(code_of(too_big), ErrorCode::Invalid);
        let listed: FilesListResult = parse(
            hub.tool_files_list(&ctx, FilesListArgs { prefix: None })
                .await
                .unwrap(),
        );
        assert_eq!(listed.files.len(), 1);
    }

    // Search ----------------------------------------------------------------

    #[tokio::test]
    async fn search_hits_depend_on_the_audience() {
        let (hub, _dir) = test_hub().await;
        let tutti = tutti(&hub).await;
        let a = new_channel(&hub, "A").await;
        let b = new_channel(&hub, "B").await;
        let tutti_ctx = ctx_for(&hub, &tutti, SessionRole::Coordinator).await;
        let a_ctx = ctx_for(&hub, &a, SessionRole::Coordinator).await;
        let b_ctx = ctx_for(&hub, &b, SessionRole::Coordinator).await;
        let task_ctx = ctx_for(&hub, &a, SessionRole::Task).await;

        user_message(&hub, a.id, "pelican in A").await;
        user_message(&hub, b.id, "pelican in B").await;
        make_task(&hub, a.id, "Feed the pelican", TaskStatus::Working).await;
        make_task(&hub, b.id, "Count pelicans", TaskStatus::Working).await;
        write_note(&hub, &tutti_ctx, "birds.md", "The user likes pelicans.").await;
        write_note(&hub, &a_ctx, "a.md", "Pelican rules for A.").await;
        write_note(&hub, &b_ctx, "b.md", "Pelican rules for B.").await;

        let search = |ctx: &ToolCtx, channel_id: Option<ChannelId>| {
            let args = SearchArgs {
                query: "pelican".to_owned(),
                channel_id,
            };
            let ctx = ctx.clone();
            let hub = hub.clone();
            async move {
                hub.tool_search(&ctx, args)
                    .await
                    .map(|v| parse::<SearchResult>(v).hits)
            }
        };
        #[derive(Debug, Default, PartialEq)]
        struct Counts {
            messages: Vec<ChannelId>,
            tasks: Vec<ChannelId>,
            notes: Vec<MemoryScope>,
        }
        let counts = |hits: Vec<SearchHit>| {
            let mut counts = Counts::default();
            for hit in hits {
                match hit {
                    SearchHit::Message { channel_id, .. } => counts.messages.push(channel_id),
                    SearchHit::Task { channel_id, .. } => counts.tasks.push(channel_id),
                    SearchHit::MemoryNote { scope, .. } => counts.notes.push(scope),
                }
            }
            counts.messages.sort();
            counts.tasks.sort();
            counts.notes.sort();
            counts
        };
        let sorted = |mut ids: Vec<ChannelId>| {
            ids.sort();
            ids
        };

        // Tutti sees every channel and every memory scope.
        let all = counts(search(&tutti_ctx, None).await.unwrap());
        assert_eq!(all.messages, sorted(vec![a.id, b.id]));
        assert_eq!(all.tasks, sorted(vec![a.id, b.id]));
        assert_eq!(all.notes.len(), 3);
        let only_b = counts(search(&tutti_ctx, Some(b.id)).await.unwrap());
        assert_eq!(only_b.messages, [b.id]);
        assert_eq!(only_b.tasks, [b.id]);
        assert_eq!(only_b.notes, [MemoryScope::Channel(b.id)]);

        // A channel coordinator and a task session see their channel and
        // global memory only.
        for ctx in [&a_ctx, &task_ctx] {
            let own = counts(search(ctx, None).await.unwrap());
            assert_eq!(own.messages, [a.id]);
            assert_eq!(own.tasks, [a.id]);
            let mut want = vec![MemoryScope::Global, MemoryScope::Channel(a.id)];
            want.sort();
            assert_eq!(own.notes, want);
            let named = counts(search(ctx, Some(a.id)).await.unwrap());
            assert_eq!(named.messages, [a.id]);
            // Another channel is refused, also when it has hits.
            let denied = search(ctx, Some(b.id)).await;
            assert_eq!(code_of(denied), ErrorCode::PolicyDenied);
        }
    }

    // The Tutti coordinator ---------------------------------------------

    #[tokio::test]
    async fn list_channels_counts_active_tasks_and_flags_archived_channels() {
        let (hub, _dir) = test_hub().await;
        let tutti = tutti(&hub).await;
        let a = new_channel(&hub, "A").await;
        let old = new_channel(&hub, "Old").await;
        hub.channel_archive(
            &Caller::Hub,
            consort_proto::client_api::ChannelArchive {
                channel_id: old.id,
                archived: true,
            },
        )
        .await
        .unwrap();
        make_task(&hub, a.id, "One", TaskStatus::Working).await;
        make_task(&hub, a.id, "Two", TaskStatus::Review).await;
        make_task(&hub, a.id, "Three", TaskStatus::Resolved).await;
        let ctx = ctx_for(&hub, &tutti, SessionRole::Coordinator).await;

        let listed: ListChannelsResult =
            parse(hub.tool_list_channels(&ctx, NoArgs {}).await.unwrap());
        assert_eq!(listed.channels.len(), 3);
        let find = |id: ChannelId| listed.channels.iter().find(|c| c.id == id).unwrap();
        assert_eq!(
            find(tutti.id).kind,
            consort_proto::model::ChannelKind::Tutti
        );
        assert!(!find(a.id).archived);
        assert_eq!(find(a.id).purpose, "About A");
        assert_eq!(find(a.id).tasks.working, 1);
        assert_eq!(find(a.id).tasks.review, 1);
        assert_eq!(find(a.id).tasks.active(), 2);
        assert!(find(old.id).archived);
        assert_eq!(find(old.id).tasks, TaskCounts::default());
    }

    #[tokio::test]
    async fn read_channel_returns_timeline_tasks_and_memory() {
        let (hub, _dir) = test_hub().await;
        let tutti = tutti(&hub).await;
        let a = new_channel(&hub, "A").await;
        let a_ctx = ctx_for(&hub, &a, SessionRole::Coordinator).await;
        let ctx = ctx_for(&hub, &tutti, SessionRole::Coordinator).await;
        let first = user_message(&hub, a.id, "first").await;
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        let second = user_message(&hub, a.id, "second").await;
        let reply = parse::<MessageIdResult>(
            hub.tool_reply(
                &a_ctx,
                ReplyArgs {
                    message_id: first.id,
                    body: "in a thread".to_owned(),
                    in_timeline: false,
                    attachments: Vec::new(),
                    suggestions: Vec::new(),
                },
            )
            .await
            .unwrap(),
        );
        let working = make_task(&hub, a.id, "Working", TaskStatus::Working).await;
        make_task(&hub, a.id, "Done", TaskStatus::Resolved).await;
        write_note(&hub, &a_ctx, "a.md", "Notes").await;

        let read = |since: Option<Timestamp>| {
            let args = ReadChannelArgs {
                channel_id: a.id,
                since,
            };
            let ctx = ctx.clone();
            let hub = hub.clone();
            async move {
                hub.tool_read_channel(&ctx, args)
                    .await
                    .map(parse::<ReadChannelResult>)
            }
        };
        let result = read(None).await.unwrap();
        assert_eq!(result.channel.id, a.id);
        // Oldest first. A thread reply is not in the timeline.
        let bodies: Vec<_> = result
            .messages
            .iter()
            .filter(|m| m.kind == MessageKind::Text)
            .map(|m| m.body.as_str())
            .collect();
        assert_eq!(bodies, ["first", "second"]);
        assert!(result.messages.iter().all(|m| m.id != reply.message_id));
        assert!(result.messages.windows(2).all(|w| w[0].id < w[1].id));
        let task_titles: Vec<_> = result.tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(task_titles, ["Working"]);
        assert_eq!(result.tasks[0].id, working.id);
        assert_eq!(result.memory.len(), 1);
        assert_eq!(result.memory[0].scope, MemoryScope::Channel(a.id));

        // `since` keeps the messages posted after that time.
        let since = read(Some(second.created_at)).await.unwrap();
        assert!(
            since
                .messages
                .iter()
                .all(|m| m.created_at > second.created_at)
        );
        assert!(
            since
                .messages
                .iter()
                .all(|m| m.id != first.id && m.id != second.id)
        );

        // The Tutti channel reads its own memory scope, which is global.
        let own = hub
            .tool_read_channel(
                &ctx,
                ReadChannelArgs {
                    channel_id: tutti.id,
                    since: None,
                },
            )
            .await
            .map(parse::<ReadChannelResult>)
            .unwrap();
        assert!(own.memory.is_empty());
        let missing = hub
            .tool_read_channel(
                &ctx,
                ReadChannelArgs {
                    channel_id: ChannelId::new(),
                    since: None,
                },
            )
            .await;
        assert_eq!(code_of(missing), ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn message_channel_posts_and_queues_the_wake_event() {
        let (hub, _dir) = test_hub().await;
        let tutti = tutti(&hub).await;
        let a = new_channel(&hub, "A").await;
        let ctx = ctx_for(&hub, &tutti, SessionRole::Coordinator).await;

        let result = hub
            .tool_message_channel(
                &ctx,
                MessageChannelArgs {
                    channel_id: a.id,
                    body: "Check the release notes.".to_owned(),
                },
            )
            .await
            .unwrap();
        let message = message_now(&hub, parse::<MessageIdResult>(result).message_id).await;
        assert_eq!(message.channel_id, a.id);
        assert_eq!(message.author, Author::Assistant);
        assert_eq!(message.author_session_id, Some(ctx.session.id));
        assert_eq!(message.body, "Check the release notes.");

        let id = a.id;
        let wakes = hub
            .store
            .read(move |conn| pending_wakes(conn, id))
            .await
            .unwrap();
        assert_eq!(wakes.len(), 1);
        assert_eq!(
            wakes[0].1,
            WakeEvent::TuttiMessage {
                id: message.id,
                at: message.created_at,
                text: "Check the release notes.".to_owned(),
            }
        );

        // Tutti is not a target. An unknown channel and an empty body are refused.
        let to_tutti = hub
            .tool_message_channel(
                &ctx,
                MessageChannelArgs {
                    channel_id: tutti.id,
                    body: "x".to_owned(),
                },
            )
            .await;
        assert_eq!(code_of(to_tutti), ErrorCode::Conflict);
        let unknown = hub
            .tool_message_channel(
                &ctx,
                MessageChannelArgs {
                    channel_id: ChannelId::new(),
                    body: "x".to_owned(),
                },
            )
            .await;
        assert_eq!(code_of(unknown), ErrorCode::NotFound);
        let empty = hub
            .tool_message_channel(
                &ctx,
                MessageChannelArgs {
                    channel_id: a.id,
                    body: " ".to_owned(),
                },
            )
            .await;
        assert_eq!(code_of(empty), ErrorCode::Invalid);
    }

    #[tokio::test]
    async fn the_tutti_only_tools_refuse_other_sessions() {
        let (hub, _dir) = test_hub().await;
        let a = new_channel(&hub, "A").await;
        let ctx = ctx_for(&hub, &a, SessionRole::Coordinator).await;

        assert_eq!(
            code_of(hub.tool_list_channels(&ctx, NoArgs {}).await),
            ErrorCode::PolicyDenied
        );
        let read = hub
            .tool_read_channel(
                &ctx,
                ReadChannelArgs {
                    channel_id: a.id,
                    since: None,
                },
            )
            .await;
        assert_eq!(code_of(read), ErrorCode::PolicyDenied);
        let message = hub
            .tool_message_channel(
                &ctx,
                MessageChannelArgs {
                    channel_id: a.id,
                    body: "x".to_owned(),
                },
            )
            .await;
        assert_eq!(code_of(message), ErrorCode::PolicyDenied);
    }

    // The memory saved line ---------------------------------------------------

    /// The memory saved lines in a channel's timeline, oldest first, as
    /// their JSON bodies.
    async fn saved_lines(hub: &Hub, channel_id: ChannelId) -> Vec<serde_json::Value> {
        let (mut page, _) = hub
            .store
            .read(move |conn| {
                conversation::messages_page(conn, channel_id, None, None, 50, usize::MAX)
            })
            .await
            .unwrap();
        page.reverse();
        page.into_iter()
            .filter(|m| m.kind == MessageKind::Notice)
            .map(|m| serde_json::from_str::<serde_json::Value>(&m.body).unwrap())
            .filter(|body| body["kind"] == "memory_saved")
            .collect()
    }

    #[tokio::test]
    async fn a_coordinator_write_posts_one_memory_saved_line_per_note() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "house-lights").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;

        let first = write_note(&hub, &ctx, "decisions/roofline.md", "Use Lumen Co.").await;
        let lines = saved_lines(&hub, channel.id).await;
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0]["text"],
            "Saved the new memory note decisions/roofline.md"
        );
        assert_eq!(lines[0]["path"], "decisions/roofline.md");
        assert_eq!(lines[0]["scope"], channel.id.to_string());
        assert_eq!(lines[0]["version"], 1);

        // A second write of the same note right after updates the line.
        hub.tool_memory_write(
            &ctx,
            MemoryWriteArgs {
                path: "decisions/roofline.md".to_owned(),
                content: "Use Lumen Co for the front.".to_owned(),
                expected_sha256: ExpectedHash::Sha256(first.sha256),
                from_suggestion: None,
            },
        )
        .await
        .unwrap();
        let lines = saved_lines(&hub, channel.id).await;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["version"], 2);
        assert_eq!(lines[0]["created"], true);

        // A user message in between makes the next write a new line.
        user_message(&hub, channel.id, "Thanks.").await;
        write_note(&hub, &ctx, "vendors/lumen-co.md", "Lumen Co.").await;
        assert_eq!(saved_lines(&hub, channel.id).await.len(), 2);

        // A write by the user from a client posts no line.
        hub.memory_write(
            &Caller::Hub,
            consort_proto::client_api::MemoryWrite {
                scope: MemoryScope::Channel(channel.id),
                path: "house.md".to_owned(),
                content: "The house.".to_owned(),
                expected_sha256: ExpectedHash::NotExists,
            },
        )
        .await
        .unwrap();
        assert_eq!(saved_lines(&hub, channel.id).await.len(), 2);
    }

    #[tokio::test]
    async fn a_write_that_keeps_a_suggestion_names_the_task_that_made_it() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "house-lights").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let task = make_task(
            &hub,
            channel.id,
            "Price out roofline lighting",
            TaskStatus::Review,
        )
        .await;

        let result = hub
            .tool_memory_write(
                &ctx,
                MemoryWriteArgs {
                    path: "decisions/roofline.md".to_owned(),
                    content: "Use Lumen Co.".to_owned(),
                    expected_sha256: ExpectedHash::NotExists,
                    from_suggestion: Some(SuggestionSource::Task(task.id)),
                },
            )
            .await
            .unwrap();
        let _: MemoryWriteResult = parse(result);

        let line = saved_lines(&hub, channel.id).await.remove(0);
        assert_eq!(
            line["text"],
            "Saved the new memory note decisions/roofline.md, from a suggestion by Price out roofline lighting"
        );
        assert_eq!(line["suggested_by"], task.id.to_string());
        assert_eq!(line["suggested_by_name"], "Price out roofline lighting");
        let scope = MemoryScope::Channel(channel.id);
        let note = hub
            .store
            .read(move |conn| knowledge::memory_note(conn, scope, "decisions/roofline.md"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(note.suggested_by, Some(SuggestionSource::Task(task.id)));
    }

    #[tokio::test]
    async fn a_write_in_a_turn_with_one_suggestion_keeps_that_suggestion() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "house-lights").await;
        let ctx = ctx_for(&hub, &channel, SessionRole::Coordinator).await;
        let task = make_task(&hub, channel.id, "Price out", TaskStatus::Review).await;
        let suggestion = |from| WakeEvent::MemorySuggestion {
            from,
            reason: "The user chose it.".to_owned(),
            note: "Lumen Co.".to_owned(),
        };
        let open_turn = |events: Vec<WakeEvent>| {
            let channel_id = channel.id;
            let session_id = ctx.session.id;
            let hub = hub.clone();
            async move {
                hub.store
                    .write(move |tx| {
                        tx.save_coordinator(&crate::store::coordinator::CoordinatorRow {
                            channel_id,
                            session_id: Some(session_id),
                            turn_open: true,
                            turn_started_at: Some(tx.now()),
                            turn_events: events,
                        })
                    })
                    .await
                    .unwrap();
            }
        };
        let scope = MemoryScope::Channel(channel.id);
        let stored = |path: &'static str| {
            let hub = hub.clone();
            async move {
                hub.store
                    .read(move |conn| knowledge::memory_note(conn, scope, path))
                    .await
                    .unwrap()
                    .unwrap()
            }
        };

        open_turn(vec![suggestion(SuggestionSource::Task(task.id))]).await;
        write_note(&hub, &ctx, "a.md", "A.").await;
        assert_eq!(
            stored("a.md").await.suggested_by,
            Some(SuggestionSource::Task(task.id))
        );

        // Two sources in one turn: the hub cannot tell which one the write
        // keeps.
        open_turn(vec![
            suggestion(SuggestionSource::Task(task.id)),
            suggestion(SuggestionSource::Channel(channel.id)),
        ])
        .await;
        write_note(&hub, &ctx, "b.md", "B.").await;
        assert_eq!(stored("b.md").await.suggested_by, None);
    }
}
