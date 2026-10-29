//! Client API handlers for channels, groups, repos and messages.
//!
//! Every handler does its reads and the writes that depend on them inside one
//! `store.write` closure, so a request never leaves a half-done change.

use std::collections::HashSet;

use consort_proto::client_api::*;
use consort_proto::frame::Empty;
use consort_proto::ids::{ChannelId, GroupId, MessageId, RepoId};
use consort_proto::model::{
    Author, Channel, ChannelKind, Group, Message, MessageKind, Reaction, ReadMarker, Repo,
    normalize_tags,
};
use consort_proto::status::Actor;
use consort_proto::tools::SearchResult;
use consort_proto::turn_input::WakeEvent;
use rusqlite::Connection;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};
use crate::ops::flow::{Delivered, deliver_in_tx};
use crate::store::conversation::{self, NewMessage};
use crate::store::{Tx, knowledge, work};

/// The most characters in a channel or group name.
const NAME_MAX_CHARS: usize = 120;
/// The most characters in a reaction emoji.
const EMOJI_MAX_CHARS: usize = 32;
/// `message.list` page size when the request gives none, and its limit.
const MESSAGE_PAGE_DEFAULT: u32 = 50;
const MESSAGE_PAGE_MAX: u32 = 200;
/// `message.search` result count when the request gives none, and its limit.
const SEARCH_LIMIT_DEFAULT: u32 = 20;
const SEARCH_LIMIT_MAX: u32 = 100;

// Helpers ------------------------------------------------------------------

/// Trims a channel or group name and checks its length.
fn clean_name(what: &str, name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(HubError::invalid(format!(
            "The {what} name is empty. Give it a name."
        )));
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(HubError::invalid(format!(
            "The {what} name is longer than {NAME_MAX_CHARS} characters. Use a shorter name."
        )));
    }
    Ok(name.to_owned())
}

/// The most local MCP servers that a channel can allow.
const ALLOWED_SERVERS_MAX: usize = 32;
/// The longest name of a local MCP server.
const SERVER_NAME_MAX_CHARS: usize = 64;

/// Trims the names of local MCP servers that a channel allows, drops the
/// copies, and checks each name. The worker refuses the name `hub` and a
/// name with `__`, so the channel refuses them too.
fn clean_server_names(names: &[String]) -> Result<Vec<String>> {
    if names.len() > ALLOWED_SERVERS_MAX {
        return Err(HubError::invalid(format!(
            "A channel can allow at most {ALLOWED_SERVERS_MAX} local MCP servers. Remove some."
        )));
    }
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > SERVER_NAME_MAX_CHARS {
            return Err(HubError::invalid(format!(
                "A local MCP server name needs 1 to {SERVER_NAME_MAX_CHARS} characters."
            )));
        }
        if name.eq_ignore_ascii_case("hub") || name.contains("__") {
            return Err(HubError::invalid(format!(
                "The local MCP server name `{name}` cannot be used. Consort keeps `hub` for the hub tools, and a name cannot contain `__`."
            )));
        }
        if !out.iter().any(|n| n == name) {
            out.push(name.to_owned());
        }
    }
    Ok(out)
}

/// The purpose and the instructions go into every system prompt of the
/// channel, so both have a size limit.
fn check_channel_text(purpose: &str, instructions: &str) -> Result<()> {
    crate::limits::check_text("purpose", purpose, crate::limits::CHANNEL_PURPOSE_MAX_BYTES)?;
    crate::limits::check_text(
        "instructions",
        instructions,
        crate::limits::CHANNEL_INSTRUCTIONS_MAX_BYTES,
    )
}

fn to_position(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

/// The position after the last one, or 0 when there is none.
fn next_position(positions: impl Iterator<Item = i64>) -> i64 {
    positions.max().map_or(0, |last| last.saturating_add(1))
}

fn require_group(conn: &Connection, id: GroupId) -> Result<Group> {
    conversation::group(conn, id)?
        .ok_or_else(|| HubError::not_found(format!("The group {id} does not exist.")))
}

fn require_repos(conn: &Connection, ids: &[RepoId]) -> Result<()> {
    for id in ids {
        if conversation::repo(conn, *id)?.is_none() {
            return Err(HubError::not_found(format!(
                "The repo {id} does not exist. Add it with repo.create first."
            )));
        }
    }
    Ok(())
}

/// The name of a repo from the last part of its URL, without `.git`.
fn default_repo_name(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    url.rsplit(['/', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or(url)
        .to_owned()
}

/// Whether a channel sits in this group. `None` is the list of channels
/// without a group. The Everything channel is in no list.
fn in_group(channel: &Channel, group_id: Option<GroupId>) -> bool {
    channel.group_id == group_id && !channel.is_everything()
}

/// Gives the channels of one group the positions 0, 1, 2 and so on, in their
/// current order. The channel `moving` is left out. With `gap`, that place
/// stays free for it. Only a channel with a new position is saved.
fn renumber_siblings(
    tx: &mut Tx<'_>,
    group_id: Option<GroupId>,
    moving: ChannelId,
    gap: Option<usize>,
) -> Result<()> {
    let now = tx.now();
    let siblings = conversation::channels(tx.db())?
        .into_iter()
        .filter(|c| in_group(c, group_id) && c.id != moving);
    for (i, mut sibling) in siblings.enumerate() {
        let slot = to_position(match gap {
            Some(gap) if i >= gap => i + 1,
            _ => i,
        });
        if sibling.position != slot {
            sibling.position = slot;
            sibling.updated_at = now;
            tx.save_channel(&sibling)?;
        }
    }
    Ok(())
}

/// Checks that the user may change this message: their own text message.
fn require_users_text(message: &Message) -> Result<()> {
    if message.author == Author::User && message.kind == MessageKind::Text {
        Ok(())
    } else {
        Err(HubError::conflict(
            "You can change only your own text messages.",
        ))
    }
}

/// The result of the write in `message_send`.
enum Sent {
    /// The client already sent this message. Nothing changed.
    Repeat(Message),
    /// The message is new. For a message in a task thread, `delivered` says
    /// where the hub queued it for the task session, when it could.
    New {
        message: Message,
        delivered: Option<(Delivered, ChannelId)>,
    },
}

impl Hub {
    /// Adds or removes the reaction of `author` with `emoji` on a message. A
    /// message has at most one reaction for each author and emoji. A change
    /// that does nothing is not saved. When `channel_id` is set, the message
    /// must be in that channel. The client API and the `react` tool both use
    /// this.
    pub(crate) async fn set_reaction(
        &self,
        message_id: MessageId,
        channel_id: Option<ChannelId>,
        author: Author,
        emoji: &str,
        remove: bool,
    ) -> Result<()> {
        let emoji = emoji.trim().to_owned();
        if emoji.is_empty() {
            return Err(HubError::invalid("The emoji is empty. Send one emoji."));
        }
        if emoji.chars().count() > EMOJI_MAX_CHARS {
            return Err(HubError::invalid(format!(
                "The emoji is longer than {EMOJI_MAX_CHARS} characters. Send one emoji."
            )));
        }
        self.store
            .write(move |tx| {
                let mut message = conversation::message_or_404(tx.db(), message_id)?;
                if channel_id.is_some_and(|id| id != message.channel_id) {
                    return Err(HubError::not_found(format!(
                        "The message {message_id} is not in this channel."
                    )));
                }
                if message.deleted_at.is_some() {
                    return Err(HubError::conflict(
                        "This message was deleted. You cannot react to it.",
                    ));
                }
                let position = message
                    .reactions
                    .iter()
                    .position(|r| r.author == author && r.emoji == emoji);
                match (position, remove) {
                    (Some(index), true) => {
                        message.reactions.remove(index);
                    }
                    (None, false) => message.reactions.push(Reaction {
                        author,
                        emoji,
                        created_at: tx.now(),
                    }),
                    // Already added, or already gone.
                    _ => return Ok(()),
                }
                tx.save_message(&message)
            })
            .await
    }
}

// Channels -----------------------------------------------------------------

impl Hub {
    pub(crate) async fn channel_create(
        &self,
        _caller: &Caller,
        request: ChannelCreate,
    ) -> Result<ChannelResult> {
        let name = clean_name("channel", &request.name)?;
        check_channel_text(&request.purpose, &request.instructions)?;
        let tags = normalize_tags(&request.tags)?;
        let channel = self
            .store
            .write(move |tx| {
                if let Some(group_id) = request.group_id {
                    require_group(tx.db(), group_id)?;
                }
                require_repos(tx.db(), &request.repo_ids)?;
                let position = next_position(
                    conversation::channels(tx.db())?
                        .iter()
                        .filter(|c| in_group(c, request.group_id))
                        .map(|c| c.position),
                );
                let now = tx.now();
                let channel = Channel {
                    id: ChannelId::new(),
                    kind: ChannelKind::Standard,
                    name,
                    icon: request.icon,
                    color: request.color,
                    group_id: request.group_id,
                    position,
                    purpose: request.purpose,
                    instructions: request.instructions,
                    coordinator_selection: request.coordinator_selection,
                    task_selection: request.task_selection,
                    preferred_workers: Vec::new(),
                    repo_ids: request.repo_ids,
                    default_isolation: request.default_isolation.unwrap_or_default(),
                    default_approval_mode: request.default_approval_mode.unwrap_or_default(),
                    allowed_local_mcp_servers: Vec::new(),
                    memory_enabled: request.memory_enabled.unwrap_or(true),
                    tags,
                    archived_at: None,
                    created_at: now,
                    updated_at: now,
                };
                tx.save_channel(&channel)?;
                Ok(channel)
            })
            .await?;
        Ok(ChannelResult { channel })
    }

    pub(crate) async fn channel_update(
        &self,
        _caller: &Caller,
        request: ChannelUpdate,
    ) -> Result<ChannelResult> {
        check_channel_text(
            request.purpose.as_deref().unwrap_or_default(),
            request.instructions.as_deref().unwrap_or_default(),
        )?;
        let tags_in_request = request.tags.is_some();
        let channel = self
            .store
            .write(move |tx| {
                let before = conversation::channel_or_404(tx.db(), request.channel_id)?;
                let mut channel = before.clone();
                if let Some(name) = request.name {
                    let name = clean_name("channel", &name)?;
                    if channel.is_everything() && name != channel.name {
                        return Err(HubError::conflict(
                            "The Everything channel cannot be renamed.",
                        ));
                    }
                    channel.name = name;
                }
                if let Some(purpose) = request.purpose {
                    channel.purpose = purpose;
                }
                if let Some(instructions) = request.instructions {
                    channel.instructions = instructions;
                }
                if let Some(icon) = request.icon {
                    channel.icon = icon;
                }
                if let Some(color) = request.color {
                    channel.color = color;
                }
                if let Some(selection) = request.coordinator_selection {
                    channel.coordinator_selection = selection;
                }
                if let Some(selection) = request.task_selection {
                    channel.task_selection = selection;
                }
                // A worker ID is checked by its type. The worker may be
                // offline or unpaired, so the hub does not look it up.
                if let Some(workers) = request.preferred_workers {
                    channel.preferred_workers = workers;
                }
                if let Some(repo_ids) = request.repo_ids {
                    require_repos(tx.db(), &repo_ids)?;
                    channel.repo_ids = repo_ids;
                }
                if let Some(isolation) = request.default_isolation {
                    channel.default_isolation = isolation;
                }
                if let Some(mode) = request.default_approval_mode {
                    channel.default_approval_mode = mode;
                }
                if let Some(servers) = request.allowed_local_mcp_servers {
                    channel.allowed_local_mcp_servers = clean_server_names(&servers)?;
                }
                if let Some(enabled) = request.memory_enabled {
                    channel.memory_enabled = enabled;
                }
                if let Some(tags) = request.tags {
                    channel.tags = normalize_tags(&tags)?;
                }
                if channel == before {
                    return Ok(channel);
                }
                channel.updated_at = tx.now();
                tx.save_channel(&channel)?;
                Ok(channel)
            })
            .await?;
        if tags_in_request {
            // Fewer tags can make a worker qualify for work that waits.
            self.retry_waiting_work();
        }
        Ok(ChannelResult { channel })
    }

    pub(crate) async fn channel_archive(
        &self,
        _caller: &Caller,
        request: ChannelArchive,
    ) -> Result<ChannelResult> {
        let channel = self
            .store
            .write(move |tx| {
                let mut channel = conversation::channel_or_404(tx.db(), request.channel_id)?;
                if channel.is_everything() {
                    return Err(HubError::conflict(
                        "The Everything channel cannot be archived.",
                    ));
                }
                let now = tx.now();
                // Archiving twice keeps the first archive time.
                let archived_at = if request.archived {
                    channel.archived_at.or(Some(now))
                } else {
                    None
                };
                if channel.archived_at != archived_at {
                    channel.archived_at = archived_at;
                    channel.updated_at = now;
                    tx.save_channel(&channel)?;
                }
                Ok(channel)
            })
            .await?;
        Ok(ChannelResult { channel })
    }

    pub(crate) async fn channel_delete(
        &self,
        _caller: &Caller,
        request: ChannelDelete,
    ) -> Result<Empty> {
        let channel_id = request.channel_id;
        let hub = self.clone();
        // One transaction: the sessions end, the workers get `session.stop`
        // and `workspace.release`, and the channel goes. A `session.start`
        // that never went out is deleted, so it cannot start a session for
        // a channel that no longer exists.
        let fx = self
            .store
            .write(move |tx| {
                let channel = conversation::channel_or_404(tx.db(), channel_id)?;
                if channel.is_everything() {
                    return Err(HubError::conflict(
                        "The Everything channel cannot be deleted.",
                    ));
                }
                let reason = "The channel was removed.";
                let mut fx = crate::link::events::Effects::default();
                for task in work::tasks_in_channel(tx.db(), channel_id, true)? {
                    // An archived task that was released has no session and
                    // no workspace left.
                    if work::task_workspace_released(tx.db(), task.id)? {
                        continue;
                    }
                    fx.merge(super::tasks::release_task(&hub, tx, &task, reason)?);
                }
                // The coordinator sessions. Task sessions ended above.
                for session in work::open_sessions_of_channel(tx.db(), channel_id)? {
                    fx.merge(crate::link::events::end_session_by_hub(
                        &hub, tx, session.id, reason,
                    )?);
                }
                tx.purge_channel(channel_id)?;
                Ok(fx)
            })
            .await?;
        self.run_effects(fx);
        Ok(Empty {})
    }

    pub(crate) async fn channel_move(
        &self,
        _caller: &Caller,
        request: ChannelMove,
    ) -> Result<ChannelResult> {
        let channel = self
            .store
            .write(move |tx| {
                let mut channel = conversation::channel_or_404(tx.db(), request.channel_id)?;
                if channel.is_everything() {
                    return Err(HubError::conflict(
                        "The Everything channel stays at the top. It cannot move.",
                    ));
                }
                if let Some(group_id) = request.group_id {
                    require_group(tx.db(), group_id)?;
                }
                let old_group_id = channel.group_id;
                let sibling_count = conversation::channels(tx.db())?
                    .iter()
                    .filter(|c| in_group(c, request.group_id) && c.id != channel.id)
                    .count();
                // A negative position means the first place. A position past
                // the end means the last place.
                let index = usize::try_from(request.position)
                    .unwrap_or(0)
                    .min(sibling_count);
                let now = tx.now();
                renumber_siblings(tx, request.group_id, channel.id, Some(index))?;
                if old_group_id != request.group_id {
                    // Close the gap that the channel leaves behind.
                    renumber_siblings(tx, old_group_id, channel.id, None)?;
                }
                let position = to_position(index);
                if channel.group_id != request.group_id || channel.position != position {
                    channel.group_id = request.group_id;
                    channel.position = position;
                    channel.updated_at = now;
                    tx.save_channel(&channel)?;
                }
                Ok(channel)
            })
            .await?;
        Ok(ChannelResult { channel })
    }
}

// Groups -------------------------------------------------------------------

impl Hub {
    pub(crate) async fn group_create(
        &self,
        _caller: &Caller,
        request: GroupCreate,
    ) -> Result<GroupResult> {
        let name = clean_name("group", &request.name)?;
        let group = self
            .store
            .write(move |tx| {
                let position =
                    next_position(conversation::groups(tx.db())?.iter().map(|g| g.position));
                let group = Group {
                    id: GroupId::new(),
                    name,
                    position,
                    collapsed: false,
                };
                tx.save_group(&group)?;
                Ok(group)
            })
            .await?;
        Ok(GroupResult { group })
    }

    pub(crate) async fn group_update(
        &self,
        _caller: &Caller,
        request: GroupUpdate,
    ) -> Result<GroupResult> {
        let name = request
            .name
            .as_deref()
            .map(|name| clean_name("group", name))
            .transpose()?;
        let group = self
            .store
            .write(move |tx| {
                let mut group = require_group(tx.db(), request.group_id)?;
                let before = group.clone();
                if let Some(name) = name {
                    group.name = name;
                }
                if let Some(collapsed) = request.collapsed {
                    group.collapsed = collapsed;
                }
                if group != before {
                    tx.save_group(&group)?;
                }
                Ok(group)
            })
            .await?;
        Ok(GroupResult { group })
    }

    pub(crate) async fn group_delete(
        &self,
        _caller: &Caller,
        request: GroupDelete,
    ) -> Result<Empty> {
        self.store
            .write(move |tx| {
                require_group(tx.db(), request.group_id)?;
                let channels = conversation::channels(tx.db())?;
                // The channels join the end of the ungrouped list, in order.
                let mut position = next_position(
                    channels
                        .iter()
                        .filter(|c| in_group(c, None))
                        .map(|c| c.position),
                );
                let now = tx.now();
                for mut channel in channels
                    .into_iter()
                    .filter(|c| c.group_id == Some(request.group_id))
                {
                    channel.group_id = None;
                    channel.position = position;
                    channel.updated_at = now;
                    position = position.saturating_add(1);
                    tx.save_channel(&channel)?;
                }
                tx.delete_group(request.group_id)
            })
            .await?;
        Ok(Empty {})
    }

    pub(crate) async fn group_reorder(
        &self,
        _caller: &Caller,
        request: GroupReorder,
    ) -> Result<Empty> {
        self.store
            .write(move |tx| {
                let groups = conversation::groups(tx.db())?;
                let mut seen = HashSet::new();
                let mut ordered = Vec::with_capacity(groups.len());
                for id in &request.group_ids {
                    if !seen.insert(*id) {
                        return Err(HubError::invalid(format!(
                            "The group {id} is in the list twice. Send each group ID once."
                        )));
                    }
                    let group = groups.iter().find(|g| g.id == *id).ok_or_else(|| {
                        HubError::invalid(format!(
                            "The group {id} does not exist. Send only IDs of existing groups."
                        ))
                    })?;
                    ordered.push(group.clone());
                }
                // A group that the list leaves out keeps its place after the
                // listed groups.
                ordered.extend(groups.into_iter().filter(|g| !seen.contains(&g.id)));
                for (index, mut group) in ordered.into_iter().enumerate() {
                    let position = to_position(index);
                    if group.position != position {
                        group.position = position;
                        tx.save_group(&group)?;
                    }
                }
                Ok(())
            })
            .await?;
        Ok(Empty {})
    }
}

// Repos --------------------------------------------------------------------

impl Hub {
    pub(crate) async fn repo_create(
        &self,
        _caller: &Caller,
        request: RepoCreate,
    ) -> Result<RepoResult> {
        let url = request.url.trim().trim_end_matches('/').to_owned();
        if url.is_empty() {
            return Err(HubError::invalid(
                "The repo URL is empty. Give the git remote URL.",
            ));
        }
        let key = conversation::repo_url_key(&url);
        let repo = self
            .store
            .write(move |tx| {
                if let Some(existing) = conversation::repo_by_key(tx.db(), &key)? {
                    return Ok(existing);
                }
                let given = |value: Option<String>| {
                    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
                };
                let repo = Repo {
                    id: RepoId::new(),
                    name: given(request.name).unwrap_or_else(|| default_repo_name(&url)),
                    default_branch: given(request.default_branch)
                        .unwrap_or_else(|| "main".to_owned()),
                    url,
                };
                tx.save_repo(&repo)?;
                Ok(repo)
            })
            .await?;
        Ok(RepoResult { repo })
    }
}

// Messages -----------------------------------------------------------------

impl Hub {
    pub(crate) async fn message_send(
        &self,
        caller: &Caller,
        request: MessageSend,
    ) -> Result<MessageResult> {
        if request.body.trim().is_empty() && request.attachments.is_empty() {
            return Err(HubError::invalid(
                "The message is empty. Write some text or attach a file.",
            ));
        }
        let origin = match caller {
            Caller::Client(client_id) => {
                if request.client_message_id.trim().is_empty() {
                    return Err(HubError::invalid(
                        "The client_message_id is empty. Send a new unique ID with each message.",
                    ));
                }
                Some((*client_id, request.client_message_id.clone()))
            }
            Caller::Hub => None,
        };
        let channel_id = request.channel_id;
        let hub = self.clone();
        let sent = self
            .store
            .write(move |tx| {
                // A repeated send returns the stored message, even when the
                // channel is archived since.
                if let Some((client_id, key)) = &origin
                    && let Some(existing) =
                        conversation::message_by_client_id(tx.db(), *client_id, key)?
                {
                    return Ok(Sent::Repeat(existing));
                }
                let channel = conversation::channel_or_404(tx.db(), channel_id)?;
                if channel.archived_at.is_some() {
                    return Err(HubError::conflict(
                        "This channel is archived. Unarchive it before you send a message.",
                    ));
                }
                let message = tx.insert_message(NewMessage {
                    thread_root_id: request.thread_root_id,
                    in_timeline: request.in_timeline,
                    attachments: request.attachments,
                    origin,
                    ..NewMessage::new(channel_id, Author::User, MessageKind::Text, request.body)
                })?;
                // The thread root of the stored message is the first message
                // of the thread, also when the request named a reply.
                let task = match message.thread_root_id {
                    Some(root) => work::task_by_card(tx.db(), root)?,
                    None => None,
                };
                let Some(task) = task else {
                    tx.queue_wake(
                        channel_id,
                        &WakeEvent::UserMessage {
                            id: message.id,
                            thread: message.thread_root_id,
                            at: message.created_at,
                            text: message.body.clone(),
                        },
                    )?;
                    return Ok(Sent::New {
                        message,
                        delivered: None,
                    });
                };
                // The delivery to the task session goes into the same
                // transaction, so a stored message always has its turn,
                // steer or inbox entry, also when the hub stops right after.
                match deliver_in_tx(&hub, tx, task.id, &message.body, Actor::User) {
                    Ok(delivered) => Ok(Sent::New {
                        message,
                        delivered: Some((delivered, task.channel_id)),
                    }),
                    Err(HubError::Api(error)) => {
                        tx.post_notice(
                            task.channel_id,
                            Some(task.card_message_id),
                            Some(task.id),
                            "message_not_sent",
                            &error.message,
                        )?;
                        Ok(Sent::New {
                            message,
                            delivered: None,
                        })
                    }
                    Err(other) => Err(other),
                }
            })
            .await?;
        match sent {
            Sent::Repeat(message) => Ok(MessageResult { message }),
            Sent::New {
                message,
                delivered: Some((delivered, task_channel)),
            } => {
                self.pump_worker(delivered.worker_id);
                if delivered.status_changed {
                    self.kick_coordinator(task_channel);
                }
                Ok(MessageResult { message })
            }
            Sent::New {
                message,
                delivered: None,
            } => {
                // A user message queued a wake event. After a refused task
                // delivery nothing is queued, and the kick does nothing.
                self.kick_coordinator(channel_id);
                Ok(MessageResult { message })
            }
        }
    }

    pub(crate) async fn message_list(
        &self,
        _caller: &Caller,
        request: MessageList,
    ) -> Result<MessageListResult> {
        let limit = usize::try_from(
            request
                .limit
                .unwrap_or(MESSAGE_PAGE_DEFAULT)
                .clamp(1, MESSAGE_PAGE_MAX),
        )
        .unwrap_or(usize::MAX);
        let (messages, has_more) = self
            .store
            .read(move |conn| {
                conversation::channel_or_404(conn, request.channel_id)?;
                conversation::messages_page(
                    conn,
                    request.channel_id,
                    request.thread_root_id,
                    request.before,
                    limit,
                    crate::limits::MESSAGE_PAGE_MAX_BYTES,
                )
            })
            .await?;
        Ok(MessageListResult { messages, has_more })
    }

    pub(crate) async fn message_edit(
        &self,
        _caller: &Caller,
        request: MessageEdit,
    ) -> Result<MessageResult> {
        let message = self
            .store
            .write(move |tx| {
                let mut message = conversation::message_or_404(tx.db(), request.message_id)?;
                require_users_text(&message)?;
                if message.deleted_at.is_some() {
                    return Err(HubError::conflict(
                        "This message was deleted. You cannot edit it.",
                    ));
                }
                if request.body.trim().is_empty() && message.attachments.is_empty() {
                    return Err(HubError::invalid(
                        "The new text is empty. Write some text, or delete the message.",
                    ));
                }
                if message.body != request.body {
                    crate::limits::check_text(
                        "message",
                        &request.body,
                        crate::limits::MESSAGE_BODY_MAX_BYTES,
                    )?;
                    message.body = request.body;
                    message.edited_at = Some(tx.now());
                    tx.save_message(&message)?;
                }
                Ok(message)
            })
            .await?;
        Ok(MessageResult { message })
    }

    pub(crate) async fn message_delete(
        &self,
        _caller: &Caller,
        request: MessageDelete,
    ) -> Result<Empty> {
        self.store
            .write(move |tx| {
                let mut message = conversation::message_or_404(tx.db(), request.message_id)?;
                require_users_text(&message)?;
                // A repeated delete does nothing.
                if message.deleted_at.is_none() {
                    message.deleted_at = Some(tx.now());
                    message.body.clear();
                    message.attachments.clear();
                    tx.save_message(&message)?;
                }
                Ok(())
            })
            .await?;
        Ok(Empty {})
    }

    pub(crate) async fn message_react(
        &self,
        _caller: &Caller,
        request: MessageReact,
    ) -> Result<Empty> {
        self.set_reaction(
            request.message_id,
            None,
            Author::User,
            &request.emoji,
            request.remove,
        )
        .await?;
        Ok(Empty {})
    }

    pub(crate) async fn message_search(
        &self,
        _caller: &Caller,
        request: MessageSearch,
    ) -> Result<SearchResult> {
        let limit = usize::try_from(
            request
                .limit
                .unwrap_or(SEARCH_LIMIT_DEFAULT)
                .clamp(1, SEARCH_LIMIT_MAX),
        )
        .unwrap_or(usize::MAX);
        let hits = self
            .store
            .read(move |conn| {
                knowledge::search(conn, &request.query, request.channel_id, limit, &|_| true)
            })
            .await?;
        Ok(SearchResult { hits })
    }

    pub(crate) async fn read_mark(&self, _caller: &Caller, request: ReadMark) -> Result<Empty> {
        self.store
            .write(move |tx| {
                let message = conversation::message(tx.db(), request.last_read_message_id)?
                    .filter(|m| m.channel_id == request.channel_id)
                    .ok_or_else(|| {
                        HubError::not_found(format!(
                            "The message {} is not in this channel.",
                            request.last_read_message_id
                        ))
                    })?;
                if let Some(root_id) = request.thread_root_id {
                    conversation::message(tx.db(), root_id)?
                        .filter(|m| m.channel_id == request.channel_id)
                        .ok_or_else(|| {
                            HubError::not_found(format!(
                                "The thread root {root_id} is not in this channel."
                            ))
                        })?;
                }
                // Message IDs are UUIDv7, so a larger ID is a newer message.
                // A marker never moves back.
                let current =
                    conversation::read_marker(tx.db(), request.channel_id, request.thread_root_id)?;
                if current.is_some_and(|c| c.last_read_message_id >= message.id) {
                    return Ok(());
                }
                tx.save_read_marker(&ReadMarker {
                    channel_id: request.channel_id,
                    thread_root_id: request.thread_root_id,
                    last_read_message_id: message.id,
                })
            })
            .await?;
        Ok(Empty {})
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use consort_proto::error::ErrorCode;
    use consort_proto::ids::{ClientId, TaskId};
    use consort_proto::model::{BlobRef, Task, TaskCreator};
    use consort_proto::status::TaskStatus;

    use super::*;
    use crate::hub::test_hub;
    use crate::store::coordinator::pending_wakes;

    fn client() -> Caller {
        Caller::Client(ClientId::new())
    }

    fn create_request(name: &str) -> ChannelCreate {
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
        }
    }

    async fn new_channel(hub: &Hub, name: &str) -> Channel {
        hub.channel_create(&Caller::Hub, create_request(name))
            .await
            .unwrap()
            .channel
    }

    async fn new_channel_in(hub: &Hub, name: &str, group_id: Option<GroupId>) -> Channel {
        let mut request = create_request(name);
        request.group_id = group_id;
        hub.channel_create(&Caller::Hub, request)
            .await
            .unwrap()
            .channel
    }

    async fn new_group(hub: &Hub, name: &str) -> Group {
        hub.group_create(
            &Caller::Hub,
            GroupCreate {
                name: name.to_owned(),
            },
        )
        .await
        .unwrap()
        .group
    }

    async fn everything(hub: &Hub) -> Channel {
        hub.store
            .read(conversation::everything_channel)
            .await
            .unwrap()
    }

    async fn channel_now(hub: &Hub, id: ChannelId) -> Channel {
        hub.store
            .read(move |conn| conversation::channel_or_404(conn, id))
            .await
            .unwrap()
    }

    fn send_request(channel_id: ChannelId, key: &str, body: &str) -> MessageSend {
        MessageSend {
            channel_id,
            client_message_id: key.to_owned(),
            body: body.to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        }
    }

    async fn send(hub: &Hub, caller: &Caller, request: MessageSend) -> Message {
        hub.message_send(caller, request).await.unwrap().message
    }

    async fn wakes(hub: &Hub, channel_id: ChannelId) -> Vec<WakeEvent> {
        hub.store
            .read(move |conn| pending_wakes(conn, channel_id))
            .await
            .unwrap()
            .into_iter()
            .map(|(_, event)| event)
            .collect()
    }

    async fn message_now(hub: &Hub, id: MessageId) -> Message {
        hub.store
            .read(move |conn| conversation::message_or_404(conn, id))
            .await
            .unwrap()
    }

    fn code_of<T: std::fmt::Debug>(result: Result<T>) -> ErrorCode {
        result.unwrap_err().code()
    }

    async fn ids_in_group(hub: &Hub, group_id: Option<GroupId>) -> Vec<(String, i64)> {
        hub.store
            .read(conversation::channels)
            .await
            .unwrap()
            .into_iter()
            .filter(|c| c.group_id == group_id && !c.is_everything())
            .map(|c| (c.name, c.position))
            .collect()
    }

    #[tokio::test]
    async fn channel_create_sets_defaults_and_the_next_position() {
        let (hub, _dir) = test_hub().await;
        let group = new_group(&hub, "Work").await;
        let first = new_channel_in(&hub, "  Sayso  ", Some(group.id)).await;
        let second = new_channel_in(&hub, "Hearth", Some(group.id)).await;
        let ungrouped = new_channel(&hub, "Notes").await;

        assert_eq!(first.name, "Sayso");
        assert_eq!(first.kind, ChannelKind::Standard);
        assert_eq!(
            first.default_isolation,
            consort_proto::model::Isolation::Worktree
        );
        assert_eq!(
            first.default_approval_mode,
            consort_proto::model::ApprovalMode::Ask
        );
        assert!(first.memory_enabled);
        assert!(first.archived_at.is_none());
        assert_eq!((first.position, second.position), (0, 1));
        assert_eq!(ungrouped.position, 0);
        assert_eq!(channel_now(&hub, second.id).await, second);
    }

    #[tokio::test]
    async fn channel_create_checks_its_input() {
        let (hub, _dir) = test_hub().await;
        for name in ["", "   ", &"a".repeat(121)] {
            let result = hub.channel_create(&Caller::Hub, create_request(name)).await;
            assert_eq!(code_of(result), ErrorCode::Invalid, "{name:?}");
        }
        let ok = hub
            .channel_create(&Caller::Hub, create_request(&"a".repeat(120)))
            .await;
        assert!(ok.is_ok());

        let mut request = create_request("x");
        request.group_id = Some(GroupId::new());
        let result = hub.channel_create(&Caller::Hub, request).await;
        assert_eq!(code_of(result), ErrorCode::NotFound);

        let mut request = create_request("x");
        request.repo_ids = vec![RepoId::new()];
        let result = hub.channel_create(&Caller::Hub, request).await;
        assert_eq!(code_of(result), ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn channel_update_changes_only_the_fields_that_it_gets() {
        let (hub, _dir) = test_hub().await;
        let mut request = create_request("Sayso");
        request.icon = Some("code".to_owned());
        request.purpose = "Ship the app".to_owned();
        let channel = hub
            .channel_create(&Caller::Hub, request)
            .await
            .unwrap()
            .channel;
        let repo = hub
            .repo_create(
                &Caller::Hub,
                RepoCreate {
                    url: "https://github.com/watzon/sayso".to_owned(),
                    name: None,
                    default_branch: None,
                },
            )
            .await
            .unwrap()
            .repo;
        tokio::time::sleep(Duration::from_millis(3)).await;

        let mut update = ChannelUpdate::new(channel.id);
        update.name = Some(" Sayso app ".to_owned());
        update.repo_ids = Some(vec![repo.id]);
        update.memory_enabled = Some(false);
        let updated = hub
            .channel_update(&Caller::Hub, update)
            .await
            .unwrap()
            .channel;
        assert_eq!(updated.name, "Sayso app");
        assert_eq!(updated.repo_ids, vec![repo.id]);
        assert!(!updated.memory_enabled);
        // Fields that the request left out stay.
        assert_eq!(updated.icon.as_deref(), Some("code"));
        assert_eq!(updated.purpose, "Ship the app");
        assert!(updated.updated_at > channel.updated_at);
        assert_eq!(updated.created_at, channel.created_at);

        // `Some(None)` clears a field.
        let mut update = ChannelUpdate::new(channel.id);
        update.icon = Some(None);
        let cleared = hub
            .channel_update(&Caller::Hub, update)
            .await
            .unwrap()
            .channel;
        assert_eq!(cleared.icon, None);
        assert_eq!(cleared.name, "Sayso app");

        // An update that changes nothing keeps `updated_at`.
        let same = hub
            .channel_update(&Caller::Hub, ChannelUpdate::new(channel.id))
            .await
            .unwrap()
            .channel;
        assert_eq!(same, cleared);

        let mut update = ChannelUpdate::new(channel.id);
        update.repo_ids = Some(vec![RepoId::new()]);
        assert_eq!(
            code_of(hub.channel_update(&Caller::Hub, update).await),
            ErrorCode::NotFound
        );
        let mut update = ChannelUpdate::new(channel.id);
        update.name = Some(" ".to_owned());
        assert_eq!(
            code_of(hub.channel_update(&Caller::Hub, update).await),
            ErrorCode::Invalid
        );
        let update = ChannelUpdate::new(ChannelId::new());
        assert_eq!(
            code_of(hub.channel_update(&Caller::Hub, update).await),
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn the_everything_channel_cannot_be_renamed_archived_deleted_or_moved() {
        let (hub, _dir) = test_hub().await;
        let everything = everything(&hub).await;
        let group = new_group(&hub, "Work").await;

        let mut update = ChannelUpdate::new(everything.id);
        update.name = Some("Home".to_owned());
        assert_eq!(
            code_of(hub.channel_update(&Caller::Hub, update).await),
            ErrorCode::Conflict
        );
        let archive = ChannelArchive {
            channel_id: everything.id,
            archived: true,
        };
        assert_eq!(
            code_of(hub.channel_archive(&Caller::Hub, archive).await),
            ErrorCode::Conflict
        );
        let delete = ChannelDelete {
            channel_id: everything.id,
        };
        assert_eq!(
            code_of(hub.channel_delete(&Caller::Hub, delete).await),
            ErrorCode::Conflict
        );
        let moved = ChannelMove {
            channel_id: everything.id,
            group_id: Some(group.id),
            position: 0,
        };
        assert_eq!(
            code_of(hub.channel_move(&Caller::Hub, moved).await),
            ErrorCode::Conflict
        );

        // Other fields and the same name are fine.
        let mut update = ChannelUpdate::new(everything.id);
        update.name = Some("Everything".to_owned());
        update.purpose = Some("Ask me anything".to_owned());
        let updated = hub
            .channel_update(&Caller::Hub, update)
            .await
            .unwrap()
            .channel;
        assert_eq!(updated.name, "Everything");
        assert_eq!(updated.purpose, "Ask me anything");
        assert_eq!(channel_now(&hub, everything.id).await.group_id, None);
    }

    #[tokio::test]
    async fn channel_archive_sets_and_clears_the_archive_time() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Old").await;
        let archive = |archived| ChannelArchive {
            channel_id: channel.id,
            archived,
        };

        let first = hub
            .channel_archive(&Caller::Hub, archive(true))
            .await
            .unwrap()
            .channel;
        assert!(first.archived_at.is_some());
        tokio::time::sleep(Duration::from_millis(3)).await;
        let again = hub
            .channel_archive(&Caller::Hub, archive(true))
            .await
            .unwrap()
            .channel;
        assert_eq!(again.archived_at, first.archived_at);

        let restored = hub
            .channel_archive(&Caller::Hub, archive(false))
            .await
            .unwrap()
            .channel;
        assert_eq!(restored.archived_at, None);
        assert_eq!(channel_now(&hub, channel.id).await.archived_at, None);
    }

    #[tokio::test]
    async fn channel_delete_removes_the_channel_with_its_messages() {
        let (hub, _dir) = test_hub().await;
        let doomed = new_channel(&hub, "Doomed").await;
        let kept = new_channel(&hub, "Kept").await;
        let user = client();
        let doomed_message = send(&hub, &user, send_request(doomed.id, "k1", "hello")).await;
        let kept_message = send(&hub, &user, send_request(kept.id, "k2", "hello")).await;

        hub.channel_delete(
            &Caller::Hub,
            ChannelDelete {
                channel_id: doomed.id,
            },
        )
        .await
        .unwrap();

        let (gone, messages, wake) = hub
            .store
            .read(move |conn| {
                Ok((
                    conversation::channel(conn, doomed.id)?,
                    conversation::message(conn, doomed_message.id)?,
                    pending_wakes(conn, doomed.id)?,
                ))
            })
            .await
            .unwrap();
        assert!(gone.is_none() && messages.is_none() && wake.is_empty());
        assert_eq!(message_now(&hub, kept_message.id).await.body, "hello");
        let again = ChannelDelete {
            channel_id: doomed.id,
        };
        assert_eq!(
            code_of(hub.channel_delete(&Caller::Hub, again).await),
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn channel_move_places_the_channel_and_renumbers_its_siblings() {
        let (hub, _dir) = test_hub().await;
        let work = new_group(&hub, "Work").await;
        let home = new_group(&hub, "Home").await;
        let a = new_channel_in(&hub, "a", Some(work.id)).await;
        let _b = new_channel_in(&hub, "b", Some(work.id)).await;
        let _c = new_channel_in(&hub, "c", Some(work.id)).await;
        let x = new_channel_in(&hub, "x", Some(home.id)).await;

        // Move `a` to the middle of its own group.
        let moved = hub
            .channel_move(
                &Caller::Hub,
                ChannelMove {
                    channel_id: a.id,
                    group_id: Some(work.id),
                    position: 1,
                },
            )
            .await
            .unwrap()
            .channel;
        assert_eq!(moved.position, 1);
        assert_eq!(
            ids_in_group(&hub, Some(work.id)).await,
            [("b".into(), 0), ("a".into(), 1), ("c".into(), 2)]
        );

        // Move `a` to another group, at a position past the end.
        hub.channel_move(
            &Caller::Hub,
            ChannelMove {
                channel_id: a.id,
                group_id: Some(home.id),
                position: 99,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            ids_in_group(&hub, Some(work.id)).await,
            [("b".into(), 0), ("c".into(), 1)]
        );
        assert_eq!(
            ids_in_group(&hub, Some(home.id)).await,
            [("x".into(), 0), ("a".into(), 1)]
        );

        // Move `a` out of every group, to the first place.
        let ungrouped = new_channel(&hub, "u").await;
        let out = hub
            .channel_move(
                &Caller::Hub,
                ChannelMove {
                    channel_id: a.id,
                    group_id: None,
                    position: -5,
                },
            )
            .await
            .unwrap()
            .channel;
        assert_eq!((out.group_id, out.position), (None, 0));
        assert_eq!(
            ids_in_group(&hub, None).await,
            [("a".into(), 0), ("u".into(), 1)]
        );
        assert_eq!(channel_now(&hub, ungrouped.id).await.position, 1);
        assert_eq!(channel_now(&hub, x.id).await.position, 0);

        let unknown = ChannelMove {
            channel_id: a.id,
            group_id: Some(GroupId::new()),
            position: 0,
        };
        assert_eq!(
            code_of(hub.channel_move(&Caller::Hub, unknown).await),
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn groups_can_be_created_updated_reordered_and_deleted() {
        let (hub, _dir) = test_hub().await;
        let a = new_group(&hub, "A").await;
        let b = new_group(&hub, "B").await;
        let c = new_group(&hub, "C").await;
        assert_eq!((a.position, b.position, c.position), (0, 1, 2));
        assert_eq!(
            code_of(
                hub.group_create(&Caller::Hub, GroupCreate { name: " ".into() })
                    .await
            ),
            ErrorCode::Invalid
        );

        let updated = hub
            .group_update(
                &Caller::Hub,
                GroupUpdate {
                    group_id: b.id,
                    name: Some(" Bee ".into()),
                    collapsed: Some(true),
                },
            )
            .await
            .unwrap()
            .group;
        assert_eq!((updated.name.as_str(), updated.collapsed), ("Bee", true));
        let only_name = hub
            .group_update(
                &Caller::Hub,
                GroupUpdate {
                    group_id: b.id,
                    name: None,
                    collapsed: None,
                },
            )
            .await
            .unwrap()
            .group;
        assert_eq!(only_name, updated);

        hub.group_reorder(
            &Caller::Hub,
            GroupReorder {
                group_ids: vec![c.id, a.id, b.id],
            },
        )
        .await
        .unwrap();
        let order = |groups: Vec<Group>| groups.into_iter().map(|g| g.name).collect::<Vec<_>>();
        let groups = hub.store.read(conversation::groups).await.unwrap();
        assert_eq!(order(groups), ["C", "A", "Bee"]);

        let unknown = GroupReorder {
            group_ids: vec![c.id, GroupId::new()],
        };
        assert_eq!(
            code_of(hub.group_reorder(&Caller::Hub, unknown).await),
            ErrorCode::Invalid
        );
        let twice = GroupReorder {
            group_ids: vec![c.id, c.id],
        };
        assert_eq!(
            code_of(hub.group_reorder(&Caller::Hub, twice).await),
            ErrorCode::Invalid
        );
        // A group that the list leaves out goes last.
        hub.group_reorder(
            &Caller::Hub,
            GroupReorder {
                group_ids: vec![b.id],
            },
        )
        .await
        .unwrap();
        let groups = hub.store.read(conversation::groups).await.unwrap();
        assert_eq!(order(groups), ["Bee", "C", "A"]);

        // Deleting a group ungroups its channels, after the ungrouped ones.
        let ungrouped = new_channel(&hub, "u").await;
        let in_a1 = new_channel_in(&hub, "a1", Some(a.id)).await;
        let in_a2 = new_channel_in(&hub, "a2", Some(a.id)).await;
        hub.group_delete(&Caller::Hub, GroupDelete { group_id: a.id })
            .await
            .unwrap();
        let (group, a1, a2) = (
            hub.store
                .read(move |conn| conversation::group(conn, a.id))
                .await
                .unwrap(),
            channel_now(&hub, in_a1.id).await,
            channel_now(&hub, in_a2.id).await,
        );
        assert!(group.is_none());
        assert_eq!((a1.group_id, a2.group_id), (None, None));
        assert_eq!(
            ids_in_group(&hub, None).await,
            [("u".into(), 0), ("a1".into(), 1), ("a2".into(), 2)]
        );
        assert_eq!(channel_now(&hub, ungrouped.id).await.position, 0);
        assert_eq!(
            code_of(
                hub.group_delete(&Caller::Hub, GroupDelete { group_id: a.id })
                    .await
            ),
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn repo_create_returns_the_existing_repo_for_the_same_remote() {
        let (hub, _dir) = test_hub().await;
        let request = |url: &str| RepoCreate {
            url: url.to_owned(),
            name: None,
            default_branch: None,
        };
        let first = hub
            .repo_create(&Caller::Hub, request("git@github.com:watzon/sayso.git"))
            .await
            .unwrap()
            .repo;
        assert_eq!(first.name, "sayso");
        assert_eq!(first.default_branch, "main");

        let same = hub
            .repo_create(&Caller::Hub, request("https://github.com/watzon/Sayso/"))
            .await
            .unwrap()
            .repo;
        assert_eq!(same, first);
        let repos = hub.store.read(conversation::repos).await.unwrap();
        assert_eq!(repos.len(), 1);

        let named = hub
            .repo_create(
                &Caller::Hub,
                RepoCreate {
                    url: "https://github.com/watzon/hearth.git".to_owned(),
                    name: Some(" Hearth app ".to_owned()),
                    default_branch: Some("trunk".to_owned()),
                },
            )
            .await
            .unwrap()
            .repo;
        assert_eq!(
            (named.name.as_str(), named.default_branch.as_str()),
            ("Hearth app", "trunk")
        );
        assert_eq!(
            code_of(hub.repo_create(&Caller::Hub, request("  ")).await),
            ErrorCode::Invalid
        );
    }

    #[tokio::test]
    async fn message_send_is_idempotent_for_a_client_message_id() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let user = client();

        let first = send(&hub, &user, send_request(channel.id, "same", "hello")).await;
        let second = send(&hub, &user, send_request(channel.id, "same", "hello again")).await;
        assert_eq!(second, first);
        assert_eq!(second.body, "hello");

        let page = hub
            .message_list(
                &user,
                MessageList {
                    channel_id: channel.id,
                    thread_root_id: None,
                    before: None,
                    limit: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(wakes(&hub, channel.id).await.len(), 1);

        // The same key from another client is another message.
        let other = send(&hub, &client(), send_request(channel.id, "same", "hi")).await;
        assert_ne!(other.id, first.id);
        // A repeat still works after the channel is archived.
        hub.channel_archive(
            &Caller::Hub,
            ChannelArchive {
                channel_id: channel.id,
                archived: true,
            },
        )
        .await
        .unwrap();
        let repeat = send(&hub, &user, send_request(channel.id, "same", "x")).await;
        assert_eq!(repeat.id, first.id);
    }

    #[tokio::test]
    async fn message_send_checks_its_input() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let user = client();

        let empty = hub
            .message_send(&user, send_request(channel.id, "k1", "  "))
            .await;
        assert_eq!(code_of(empty), ErrorCode::Invalid);
        let no_key = hub
            .message_send(&user, send_request(channel.id, " ", "hi"))
            .await;
        assert_eq!(code_of(no_key), ErrorCode::Invalid);
        let missing = hub
            .message_send(&user, send_request(ChannelId::new(), "k2", "hi"))
            .await;
        assert_eq!(code_of(missing), ErrorCode::NotFound);

        // An attachment alone is a message.
        let mut request = send_request(channel.id, "k3", "");
        request.attachments = vec![BlobRef {
            sha256: crate::util::sha256(b"png"),
            name: "a.png".to_owned(),
            mime: "image/png".to_owned(),
            size: 3,
        }];
        let sent = send(&hub, &user, request).await;
        assert_eq!(sent.attachments.len(), 1);

        hub.channel_archive(
            &Caller::Hub,
            ChannelArchive {
                channel_id: channel.id,
                archived: true,
            },
        )
        .await
        .unwrap();
        let archived = hub
            .message_send(&user, send_request(channel.id, "k4", "hi"))
            .await;
        assert_eq!(code_of(archived), ErrorCode::Conflict);
    }

    #[tokio::test]
    async fn a_user_message_queues_exactly_one_wake_event() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let message = send(&hub, &client(), send_request(channel.id, "k1", "hello")).await;

        let events = wakes(&hub, channel.id).await;
        assert_eq!(
            events,
            [WakeEvent::UserMessage {
                id: message.id,
                thread: None,
                at: message.created_at,
                text: "hello".to_owned(),
            }]
        );
        // The hub as a caller sends without an idempotency key.
        let from_hub = send(&hub, &Caller::Hub, send_request(channel.id, "", "tick")).await;
        assert_eq!(from_hub.client_message_id, None);
        assert_eq!(wakes(&hub, channel.id).await.len(), 2);
    }

    #[tokio::test]
    async fn a_thread_reply_raises_the_reply_count_of_the_root() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let user = client();
        let root = send(&hub, &user, send_request(channel.id, "k1", "root")).await;
        assert_eq!(root.reply_count, 0);

        let mut reply = send_request(channel.id, "k2", "reply");
        reply.thread_root_id = Some(root.id);
        let reply = send(&hub, &user, reply).await;
        assert_eq!(reply.thread_root_id, Some(root.id));
        assert!(!reply.in_timeline);
        let mut second = send_request(channel.id, "k3", "reply to the reply");
        second.thread_root_id = Some(reply.id);
        second.in_timeline = true;
        let second = send(&hub, &user, second).await;
        // A reply to a reply joins the thread of the first message.
        assert_eq!(second.thread_root_id, Some(root.id));

        let root_now = message_now(&hub, root.id).await;
        assert_eq!(root_now.reply_count, 2);
        assert!(root_now.last_reply_at.is_some());

        let events = wakes(&hub, channel.id).await;
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[1],
            WakeEvent::UserMessage { thread: Some(t), .. } if *t == root.id
        ));

        let timeline = hub
            .message_list(
                &user,
                MessageList {
                    channel_id: channel.id,
                    thread_root_id: None,
                    before: None,
                    limit: None,
                },
            )
            .await
            .unwrap();
        let ids: Vec<_> = timeline.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, [second.id, root.id]);
        let thread = hub
            .message_list(
                &user,
                MessageList {
                    channel_id: channel.id,
                    thread_root_id: Some(root.id),
                    before: None,
                    limit: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(thread.messages.len(), 2);

        // A root in another channel is refused.
        let other = new_channel(&hub, "Other").await;
        let mut stray = send_request(other.id, "k4", "stray");
        stray.thread_root_id = Some(root.id);
        assert!(hub.message_send(&user, stray).await.is_err());
    }

    #[tokio::test]
    async fn a_message_in_a_task_card_thread_queues_no_wake_event() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let channel_id = channel.id;
        let task_id = TaskId::new();
        let card = hub
            .store
            .write(move |tx| {
                let mut new =
                    NewMessage::new(channel_id, Author::System, MessageKind::TaskCard, "");
                new.author_task_id = Some(task_id);
                let card = tx.insert_message(new)?;
                let now = tx.now();
                tx.save_task(&Task {
                    id: task_id,
                    channel_id,
                    title: "Fix the bug".to_owned(),
                    brief: "Find it".to_owned(),
                    status: TaskStatus::Working,
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
                    archived_at: None,
                    created_at: now,
                    updated_at: now,
                })?;
                Ok(card)
            })
            .await
            .unwrap();

        let mut request = send_request(channel.id, "k1", "Use the new API");
        request.thread_root_id = Some(card.id);
        // Routing to the task session is a hub hook. It may not be wired in
        // yet, so this test looks at the stored state and not at the result.
        let _ = hub.message_send(&client(), request).await;

        assert!(wakes(&hub, channel.id).await.is_empty());
        let thread = hub
            .store
            .read(move |conn| conversation::thread_replies(conn, card.id))
            .await
            .unwrap();
        // The task has no session in this test, so the hub also posts a notice.
        let from_user: Vec<_> = thread.iter().filter(|m| m.author == Author::User).collect();
        assert_eq!(from_user.len(), 1);
        assert_eq!(from_user[0].body, "Use the new API");
    }

    #[tokio::test]
    async fn message_list_pages_newest_first() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let user = client();
        let mut sent = Vec::new();
        for i in 0..5 {
            sent.push(
                send(
                    &hub,
                    &user,
                    send_request(channel.id, &format!("k{i}"), &format!("m{i}")),
                )
                .await,
            );
        }
        let list = |before, limit| MessageList {
            channel_id: channel.id,
            thread_root_id: None,
            before,
            limit,
        };
        let page = hub.message_list(&user, list(None, Some(2))).await.unwrap();
        assert_eq!(
            page.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            [sent[4].id, sent[3].id]
        );
        assert!(page.has_more);
        let rest = hub
            .message_list(&user, list(Some(sent[3].id), Some(10)))
            .await
            .unwrap();
        assert_eq!(rest.messages.len(), 3);
        assert!(!rest.has_more);
        assert_eq!(
            code_of(
                hub.message_list(
                    &user,
                    MessageList {
                        channel_id: ChannelId::new(),
                        thread_root_id: None,
                        before: None,
                        limit: None
                    }
                )
                .await
            ),
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn message_edit_and_delete_apply_to_the_users_text_messages_only() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let channel_id = channel.id;
        let user = client();
        let mine = send(&hub, &user, send_request(channel.id, "k1", "first")).await;
        let theirs = hub
            .store
            .write(move |tx| {
                tx.insert_message(NewMessage::new(
                    channel_id,
                    Author::Assistant,
                    MessageKind::Text,
                    "hi",
                ))
            })
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(3)).await;
        let edited = hub
            .message_edit(
                &user,
                MessageEdit {
                    message_id: mine.id,
                    body: "second".into(),
                },
            )
            .await
            .unwrap()
            .message;
        assert_eq!(edited.body, "second");
        assert!(edited.edited_at.is_some());
        let blank = MessageEdit {
            message_id: mine.id,
            body: " ".into(),
        };
        assert_eq!(
            code_of(hub.message_edit(&user, blank).await),
            ErrorCode::Invalid
        );
        let not_mine = MessageEdit {
            message_id: theirs.id,
            body: "x".into(),
        };
        assert_eq!(
            code_of(hub.message_edit(&user, not_mine).await),
            ErrorCode::Conflict
        );
        let delete_theirs = MessageDelete {
            message_id: theirs.id,
        };
        assert_eq!(
            code_of(hub.message_delete(&user, delete_theirs).await),
            ErrorCode::Conflict
        );

        hub.message_delete(
            &user,
            MessageDelete {
                message_id: mine.id,
            },
        )
        .await
        .unwrap();
        let deleted = message_now(&hub, mine.id).await;
        assert!(deleted.deleted_at.is_some());
        assert!(deleted.body.is_empty() && deleted.attachments.is_empty());
        // A second delete does nothing. A deleted message cannot change.
        hub.message_delete(
            &user,
            MessageDelete {
                message_id: mine.id,
            },
        )
        .await
        .unwrap();
        let again = MessageEdit {
            message_id: mine.id,
            body: "third".into(),
        };
        assert_eq!(
            code_of(hub.message_edit(&user, again).await),
            ErrorCode::Conflict
        );
        let react = MessageReact {
            message_id: mine.id,
            emoji: "x".into(),
            remove: false,
        };
        assert_eq!(
            code_of(hub.message_react(&user, react).await),
            ErrorCode::Conflict
        );
        let page = hub
            .message_list(
                &user,
                MessageList {
                    channel_id,
                    thread_root_id: None,
                    before: None,
                    limit: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.messages.len(), 1);
    }

    #[tokio::test]
    async fn message_react_keeps_one_reaction_for_each_emoji_and_author() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let user = client();
        let message = send(&hub, &user, send_request(channel.id, "k1", "hello")).await;
        let react = |emoji: &str, remove| MessageReact {
            message_id: message.id,
            emoji: emoji.to_owned(),
            remove,
        };

        hub.message_react(&user, react("👍", false)).await.unwrap();
        hub.message_react(&user, react("👍", false)).await.unwrap();
        hub.message_react(&user, react("🎉", false)).await.unwrap();
        let reactions = message_now(&hub, message.id).await.reactions;
        assert_eq!(reactions.len(), 2);
        assert!(reactions.iter().all(|r| r.author == Author::User));

        hub.message_react(&user, react("👍", true)).await.unwrap();
        hub.message_react(&user, react("👍", true)).await.unwrap();
        let reactions = message_now(&hub, message.id).await.reactions;
        assert_eq!(
            reactions
                .iter()
                .map(|r| r.emoji.as_str())
                .collect::<Vec<_>>(),
            ["🎉"]
        );

        // The assistant has its own reaction with the same emoji.
        hub.set_reaction(message.id, Some(channel.id), Author::Assistant, "🎉", false)
            .await
            .unwrap();
        assert_eq!(message_now(&hub, message.id).await.reactions.len(), 2);
        // The channel filter refuses a message of another channel.
        let other = new_channel(&hub, "Other").await;
        let result = hub
            .set_reaction(message.id, Some(other.id), Author::Assistant, "🎉", false)
            .await;
        assert_eq!(code_of(result), ErrorCode::NotFound);

        for bad in ["", "  ", &"x".repeat(33)] {
            let result = hub.message_react(&user, react(bad, false)).await;
            assert_eq!(code_of(result), ErrorCode::Invalid, "{bad:?}");
        }
        let unknown = MessageReact {
            message_id: MessageId::new(),
            emoji: "👍".into(),
            remove: false,
        };
        assert_eq!(
            code_of(hub.message_react(&user, unknown).await),
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn message_search_finds_messages_and_can_filter_by_channel() {
        let (hub, _dir) = test_hub().await;
        let a = new_channel(&hub, "A").await;
        let b = new_channel(&hub, "B").await;
        let user = client();
        send(&hub, &user, send_request(a.id, "k1", "the pelican flew")).await;
        send(&hub, &user, send_request(b.id, "k2", "another pelican")).await;
        let search = |channel_id| MessageSearch {
            query: "pelican".to_owned(),
            channel_id,
            limit: None,
        };

        let all = hub.message_search(&user, search(None)).await.unwrap();
        assert_eq!(all.hits.len(), 2);
        let only_a = hub.message_search(&user, search(Some(a.id))).await.unwrap();
        assert_eq!(only_a.hits.len(), 1);
        let one = hub
            .message_search(
                &user,
                MessageSearch {
                    limit: Some(1),
                    ..search(None)
                },
            )
            .await
            .unwrap();
        assert_eq!(one.hits.len(), 1);
    }

    #[tokio::test]
    async fn the_read_marker_only_moves_forward() {
        let (hub, _dir) = test_hub().await;
        let channel = new_channel(&hub, "Chat").await;
        let other = new_channel(&hub, "Other").await;
        let user = client();
        let first = send(&hub, &user, send_request(channel.id, "k1", "one")).await;
        let second = send(&hub, &user, send_request(channel.id, "k2", "two")).await;
        let elsewhere = send(&hub, &user, send_request(other.id, "k3", "x")).await;
        let mark = |message: &Message| ReadMark {
            channel_id: channel.id,
            thread_root_id: None,
            last_read_message_id: message.id,
        };
        let marker = |hub: &Hub| {
            let channel_id = channel.id;
            let store = hub.store.clone();
            async move {
                store
                    .read(move |conn| conversation::read_marker(conn, channel_id, None))
                    .await
                    .unwrap()
                    .map(|m| m.last_read_message_id)
            }
        };

        assert_eq!(marker(&hub).await, None);
        hub.read_mark(&user, mark(&first)).await.unwrap();
        assert_eq!(marker(&hub).await, Some(first.id));
        hub.read_mark(&user, mark(&second)).await.unwrap();
        assert_eq!(marker(&hub).await, Some(second.id));
        // An older message changes nothing.
        hub.read_mark(&user, mark(&first)).await.unwrap();
        assert_eq!(marker(&hub).await, Some(second.id));
        // The same message again is fine.
        hub.read_mark(&user, mark(&second)).await.unwrap();
        assert_eq!(marker(&hub).await, Some(second.id));

        // A message of another channel, or an unknown one, is refused.
        assert_eq!(
            code_of(hub.read_mark(&user, mark(&elsewhere)).await),
            ErrorCode::NotFound
        );
        let unknown = ReadMark {
            channel_id: channel.id,
            thread_root_id: None,
            last_read_message_id: MessageId::new(),
        };
        assert_eq!(
            code_of(hub.read_mark(&user, unknown).await),
            ErrorCode::NotFound
        );

        // A thread has its own marker.
        let mut reply = send_request(channel.id, "k4", "reply");
        reply.thread_root_id = Some(first.id);
        let reply = send(&hub, &user, reply).await;
        let thread_mark = ReadMark {
            channel_id: channel.id,
            thread_root_id: Some(first.id),
            last_read_message_id: reply.id,
        };
        hub.read_mark(&user, thread_mark).await.unwrap();
        let channel_id = channel.id;
        let thread_marker = hub
            .store
            .read(move |conn| conversation::read_marker(conn, channel_id, Some(first.id)))
            .await
            .unwrap();
        assert_eq!(
            thread_marker.map(|m| m.last_read_message_id),
            Some(reply.id)
        );
        assert_eq!(marker(&hub).await, Some(second.id));
    }
}
