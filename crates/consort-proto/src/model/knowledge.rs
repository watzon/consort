//! Knowledge entities: memory notes, memory access rules, channel files and
//! artifacts.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::common::{Sha256Hash, ValidationError, validate_relative_path};
use crate::ids::{ChannelId, SessionId, TaskId};
use crate::time::Timestamp;

/// Where a memory note lives: global memory, or one channel's memory.
/// Serialized as `"global"` or a channel ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryScope {
    /// The memory about the user. Every channel reads it. Only the Everything
    /// coordinator and the user write it. It is the Everything channel's
    /// memory.
    Global,
    /// The memory of one channel.
    Channel(ChannelId),
}

/// The error when a string is not a memory scope.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a memory scope; use `global` or a channel ID")]
pub struct ScopeParseError(pub String);

impl FromStr for MemoryScope {
    type Err = ScopeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "global" {
            return Ok(Self::Global);
        }
        s.parse()
            .map(Self::Channel)
            .map_err(|_| ScopeParseError(s.to_owned()))
    }
}

impl fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => f.write_str("global"),
            Self::Channel(id) => id.fmt(f),
        }
    }
}

impl Serialize for MemoryScope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MemoryScope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for MemoryScope {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("MemoryScope")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "`global`, or a channel ID that starts with `chn_`.",
        })
    }
}

/// Who wrote a memory note version: the user, or a session.
/// Serialized as `"user"` or a session ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryAuthor {
    /// The user, from a client.
    User,
    /// A coordinator session.
    Session(SessionId),
}

impl Serialize for MemoryAuthor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::User => serializer.serialize_str("user"),
            Self::Session(id) => id.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MemoryAuthor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        if s == "user" {
            return Ok(Self::User);
        }
        s.parse()
            .map(Self::Session)
            .map_err(serde::de::Error::custom)
    }
}

/// One markdown file in memory, addressed by its scope and path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryNote {
    /// `global` or a channel ID.
    pub scope: MemoryScope,
    /// The path, for example `people/kevin.md`.
    pub path: String,
    /// The markdown content.
    pub content: String,
    /// The hash of `content`.
    pub sha256: Sha256Hash,
    /// The version, from 1.
    pub version: u64,
    /// Who wrote this version.
    pub updated_by: MemoryAuthor,
    /// When.
    pub updated_at: Timestamp,
}

impl MemoryNote {
    /// The note without its content.
    pub fn summary(&self) -> MemoryNoteSummary {
        MemoryNoteSummary {
            scope: self.scope,
            path: self.path.clone(),
            sha256: self.sha256.clone(),
            version: self.version,
            updated_by: self.updated_by,
            updated_at: self.updated_at,
        }
    }

    /// The feed `entity_id` of this note: `<scope>:<path>`.
    pub fn entity_id(&self) -> String {
        memory_note_entity_id(self.scope, &self.path)
    }
}

/// The feed `entity_id` of a memory note: `<scope>:<path>`.
pub fn memory_note_entity_id(scope: MemoryScope, path: &str) -> String {
    format!("{scope}:{path}")
}

/// A memory note without its content, for lists and the memory index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryNoteSummary {
    /// `global` or a channel ID.
    pub scope: MemoryScope,
    /// The path.
    pub path: String,
    /// The hash of the content.
    pub sha256: Sha256Hash,
    /// The version.
    pub version: u64,
    /// Who wrote this version.
    pub updated_by: MemoryAuthor,
    /// When.
    pub updated_at: Timestamp,
}

/// The hash that a write expects: the stored note's hash, or `not_exists`
/// for a new note.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpectedHash {
    /// The note must not exist yet.
    NotExists,
    /// The stored note must have this hash.
    Sha256(Sha256Hash),
}

impl ExpectedHash {
    /// Whether the stored hash meets this expectation. A mismatch is a
    /// `stale_write` error.
    pub fn matches(&self, stored: Option<&Sha256Hash>) -> bool {
        match (self, stored) {
            (Self::NotExists, None) => true,
            (Self::Sha256(expected), Some(stored)) => expected == stored,
            _ => false,
        }
    }
}

impl Serialize for ExpectedHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::NotExists => serializer.serialize_str("not_exists"),
            Self::Sha256(hash) => hash.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ExpectedHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = Cow::<'de, str>::deserialize(deserializer)?;
        if s == "not_exists" {
            return Ok(Self::NotExists);
        }
        s.parse()
            .map(Self::Sha256)
            .map_err(|_| serde::de::Error::custom("expected a SHA-256 hash or `not_exists`"))
    }
}

impl JsonSchema for ExpectedHash {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ExpectedHash")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "The sha256 of the note that you read, or `not_exists` for a new note.",
        })
    }
}

/// Who reads or writes memory. The rules come from data-model.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryActor {
    /// The user, from a client. Reads and writes every scope.
    User,
    /// The Everything coordinator. Reads every scope. Writes `global` only.
    EverythingCoordinator,
    /// A channel coordinator. Reads `global` and its channel. Writes its
    /// channel.
    ChannelCoordinator(ChannelId),
    /// A task session. Reads `global` and the memory scope of its channel.
    /// Writes nothing. For a task in Everything, that scope is `global`.
    TaskSession(MemoryScope),
}

impl MemoryActor {
    /// Whether this actor may read notes in `scope`.
    pub fn can_read(&self, scope: MemoryScope) -> bool {
        match self {
            Self::User | Self::EverythingCoordinator => true,
            Self::ChannelCoordinator(own) => {
                scope == MemoryScope::Global || scope == MemoryScope::Channel(*own)
            }
            Self::TaskSession(own) => scope == MemoryScope::Global || scope == *own,
        }
    }

    /// Whether this actor may write notes in `scope`.
    pub fn can_write(&self, scope: MemoryScope) -> bool {
        match self {
            Self::User => true,
            Self::EverythingCoordinator => scope == MemoryScope::Global,
            Self::ChannelCoordinator(own) => scope == MemoryScope::Channel(*own),
            Self::TaskSession(_) => false,
        }
    }

    /// The scope that hub tools use when the call gives no `scope`, and the
    /// only scope that `memory_write` writes to. `None` for the user, who
    /// must always name a scope.
    pub fn own_scope(&self) -> Option<MemoryScope> {
        match self {
            Self::User => None,
            Self::EverythingCoordinator => Some(MemoryScope::Global),
            Self::ChannelCoordinator(own) => Some(MemoryScope::Channel(*own)),
            Self::TaskSession(own) => Some(*own),
        }
    }
}

/// The most bytes in a memory note path.
pub const MEMORY_PATH_MAX_BYTES: usize = 256;

/// Checks a memory note path: a safe relative path that ends in `.md`.
pub fn validate_memory_path(path: &str) -> Result<(), ValidationError> {
    validate_relative_path("path", path)?;
    if path.len() > MEMORY_PATH_MAX_BYTES {
        return Err(ValidationError::new(
            "path",
            format!("is longer than {MEMORY_PATH_MAX_BYTES} bytes"),
        ));
    }
    if !path.ends_with(".md") {
        return Err(ValidationError::new("path", "must end in `.md`"));
    }
    Ok(())
}

/// The folder in channel files where tasks publish artifacts.
pub const ARTIFACTS_DIR: &str = "artifacts";

/// The channel file path of an artifact: `artifacts/<task-id>/<file name>`.
/// Only the last segment of `workspace_path` is kept.
pub fn artifact_path(task_id: TaskId, workspace_path: &str) -> String {
    let name = workspace_path
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("artifact");
    format!("{ARTIFACTS_DIR}/{task_id}/{name}")
}

/// A file in a channel's shared folder. An artifact is a channel file with a
/// `task_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelFile {
    /// The channel.
    pub channel_id: ChannelId,
    /// The path in the channel's folder.
    pub path: String,
    /// The blob.
    pub blob_sha256: Sha256Hash,
    /// The size in bytes.
    pub size: u64,
    /// The media type.
    pub mime: String,
    /// The task that published it. Set on artifacts.
    pub task_id: Option<TaskId>,
    /// The artifact title. Set on artifacts.
    pub title: Option<String>,
    /// `user` or the session ID that wrote it.
    pub updated_by: String,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// When it was deleted.
    pub deleted_at: Option<Timestamp>,
}

impl ChannelFile {
    /// Whether this file is an artifact.
    pub fn is_artifact(&self) -> bool {
        self.task_id.is_some()
    }

    /// The feed `entity_id` of this file: `<channel-id>:<path>`.
    pub fn entity_id(&self) -> String {
        format!("{}:{}", self.channel_id, self.path)
    }
}

/// A channel file path with its hash, as `session.start` lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    /// The path in the channel's folder.
    pub path: String,
    /// The blob hash.
    pub sha256: Sha256Hash,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_serde() {
        assert_eq!(
            serde_json::to_string(&MemoryScope::Global).unwrap(),
            "\"global\""
        );
        let id = ChannelId::new();
        let scope = MemoryScope::Channel(id);
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<MemoryScope>(&json).unwrap(), scope);
        assert!(
            serde_json::from_str::<MemoryScope>("\"tsk_00000000-0000-0000-0000-000000000000\"")
                .is_err()
        );
        assert!(serde_json::from_str::<MemoryScope>("\"channel\"").is_err());
    }

    #[test]
    fn author_serde() {
        assert_eq!(
            serde_json::to_string(&MemoryAuthor::User).unwrap(),
            "\"user\""
        );
        let ses = SessionId::new();
        let json = serde_json::to_string(&MemoryAuthor::Session(ses)).unwrap();
        assert_eq!(
            serde_json::from_str::<MemoryAuthor>(&json).unwrap(),
            MemoryAuthor::Session(ses)
        );
    }

    #[test]
    fn expected_hash() {
        let hash: Sha256Hash = "b".repeat(64).parse().unwrap();
        let other: Sha256Hash = "c".repeat(64).parse().unwrap();
        assert!(ExpectedHash::NotExists.matches(None));
        assert!(!ExpectedHash::NotExists.matches(Some(&hash)));
        assert!(ExpectedHash::Sha256(hash.clone()).matches(Some(&hash)));
        assert!(!ExpectedHash::Sha256(hash.clone()).matches(Some(&other)));
        assert!(!ExpectedHash::Sha256(hash.clone()).matches(None));
        assert_eq!(
            serde_json::to_string(&ExpectedHash::NotExists).unwrap(),
            "\"not_exists\""
        );
        assert_eq!(
            serde_json::from_str::<ExpectedHash>(&format!("\"{hash}\"")).unwrap(),
            ExpectedHash::Sha256(hash)
        );
        assert!(serde_json::from_str::<ExpectedHash>("\"exists\"").is_err());
    }

    #[test]
    fn memory_access_rules() {
        let own = ChannelId::new();
        let other = ChannelId::new();
        let g = MemoryScope::Global;
        let mine = MemoryScope::Channel(own);
        let theirs = MemoryScope::Channel(other);

        let user = MemoryActor::User;
        assert!(user.can_read(theirs) && user.can_write(theirs) && user.can_write(g));

        let everything = MemoryActor::EverythingCoordinator;
        assert!(everything.can_read(g) && everything.can_read(theirs));
        assert!(everything.can_write(g));
        assert!(!everything.can_write(theirs));

        let coordinator = MemoryActor::ChannelCoordinator(own);
        assert!(coordinator.can_read(g) && coordinator.can_read(mine));
        assert!(!coordinator.can_read(theirs));
        assert!(coordinator.can_write(mine));
        assert!(!coordinator.can_write(g));
        assert!(!coordinator.can_write(theirs));

        let task = MemoryActor::TaskSession(mine);
        assert!(task.can_read(g) && task.can_read(mine));
        assert!(!task.can_read(theirs));
        assert!(!task.can_write(mine) && !task.can_write(g));

        assert_eq!(everything.own_scope(), Some(g));
        assert_eq!(coordinator.own_scope(), Some(mine));
        assert_eq!(user.own_scope(), None);
    }

    #[test]
    fn memory_paths() {
        assert!(validate_memory_path("people/kevin.md").is_ok());
        assert!(validate_memory_path("people/kevin.txt").is_err());
        assert!(validate_memory_path("../kevin.md").is_err());
        assert!(validate_memory_path(&format!("{}.md", "a".repeat(254))).is_err());
    }

    #[test]
    fn artifact_paths() {
        let task: TaskId = "tsk_00000000-0000-0000-0000-000000000009".parse().unwrap();
        assert_eq!(
            artifact_path(task, "out/report.pdf"),
            "artifacts/tsk_00000000-0000-0000-0000-000000000009/report.pdf"
        );
        assert_eq!(
            artifact_path(task, "C:\\w\\out\\chart.png"),
            "artifacts/tsk_00000000-0000-0000-0000-000000000009/chart.png"
        );
    }
}
