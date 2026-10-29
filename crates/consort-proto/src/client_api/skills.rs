//! Client requests for skills: the directories, preview and install,
//! updates, removal and the file viewer.

use serde::{Deserialize, Serialize};

use crate::ids::{ApprovalId, SkillId};
use crate::model::{
    Sha256Hash, Skill, SkillAudit, SkillDirectoryEntry, SkillDirectorySource, SkillFileInfo, UseIn,
};

/// The most results of one skill directory page.
pub const SKILL_DIRECTORY_PAGE_MAX: u32 = 100;

/// `skill_directory.search`: search the skill directories. The hub merges
/// the results of every directory, or searches one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDirectorySearch {
    /// Words to find.
    #[serde(default)]
    pub search: Option<String>,
    /// Only this directory source ID (see `skill_directory.sources`).
    #[serde(default)]
    pub source: Option<String>,
    /// The `next_cursor` of the page before.
    #[serde(default)]
    pub cursor: Option<String>,
    /// The most entries. The default is 30 and the limit is
    /// [`SKILL_DIRECTORY_PAGE_MAX`].
    #[serde(default)]
    pub limit: Option<u32>,
}

/// The response to `skill_directory.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDirectorySearchResult {
    /// The entries.
    pub entries: Vec<SkillDirectoryEntry>,
    /// The cursor of the next page. Empty on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
    /// Directories that did not answer, by name, so the dialog can say so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<String>,
}

/// `skill_directory.sources`: the directories that the hub searches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDirectorySources {}

/// `skill_directory.add_source`: add a Claude plugin `marketplace.json`
/// file. The hub reads it once to check it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDirectoryAddSource {
    /// The URL of the file, or a git repo that has
    /// `.claude-plugin/marketplace.json`.
    pub url: String,
}

/// `skill_directory.remove_source`: remove a marketplace that the user
/// added. Installed skills from it stay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDirectoryRemoveSource {
    /// The source ID.
    pub id: String,
}

/// The response to the `skill_directory.*source*` requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDirectorySourcesResult {
    /// Every source, the built-in ones first.
    pub sources: Vec<SkillDirectorySource>,
}

/// Where to install a skill from. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillInstallSource {
    /// An entry from `skill_directory.search`.
    Directory {
        /// The directory source ID. Empty for the entry's own directory.
        #[serde(default)]
        source: Option<String>,
        /// The entry ID.
        entry_id: String,
    },
    /// A git repo, or `owner/repo` for GitHub.
    Git {
        /// The clone URL or `owner/repo`.
        url: String,
        /// The folder of the skill in the repo. Empty for the repo root.
        #[serde(default)]
        path: Option<String>,
        /// The branch or tag. Empty for the default branch.
        #[serde(default, rename = "ref")]
        git_ref: Option<String>,
    },
    /// A zip file that the client uploaded first with
    /// `PUT /v1/blobs/{sha256}`, at most
    /// [`crate::model::SKILL_UPLOAD_MAX_BYTES`].
    Upload {
        /// The uploaded blob.
        blob_sha256: Sha256Hash,
        /// The size in bytes.
        size: u64,
        /// The file name, as the user gave it.
        file_name: String,
    },
}

/// `skill.preview`: fetch and check a skill without installing it, so the
/// add dialog can show the safety facts. The hub keeps the fetched files
/// for one hour, so a `skill.install` of the same source right after does
/// not fetch again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPreview {
    /// Where to fetch it from.
    pub source: SkillInstallSource,
}

/// The response to `skill.preview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPreviewResult {
    /// The `name` of its `SKILL.md`.
    pub name: String,
    /// The `description` of its `SKILL.md`.
    pub description: String,
    /// The commit that would be pinned. Empty for an upload.
    #[serde(default)]
    pub commit: Option<String>,
    /// The content hash.
    pub content_hash: Sha256Hash,
    /// The license.
    #[serde(default)]
    pub license: Option<String>,
    /// The files that can run.
    #[serde(default)]
    pub scripts: Vec<String>,
    /// The audit results.
    #[serde(default)]
    pub audits: Vec<SkillAudit>,
    /// Every file.
    pub files: Vec<SkillFileInfo>,
    /// True when it has Claude-specific `hooks`, which the hub removes.
    #[serde(default)]
    pub has_hooks: bool,
    /// Why the hub would block it.
    #[serde(default)]
    pub blocked_reason: Option<String>,
    /// The installed skill with the same name, when there is one.
    #[serde(default)]
    pub installed_skill_id: Option<SkillId>,
}

/// `skill.install`: fetch, check and install a skill. A skill whose audit
/// failed is refused with `skill_blocked`. A skill that breaks the format
/// or a limit is refused with `skill_invalid`. A skill with scripts is
/// installed off, whatever `enabled` says, until the user turns it on with
/// `skill.update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInstall {
    /// Where to fetch it from.
    pub source: SkillInstallSource,
    /// Where it can be used. The default is every channel.
    #[serde(default)]
    pub use_in: UseIn,
    /// Turn it on. The default is true for a skill with no scripts.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// The install card that this answers, when the user added the skill
    /// from a card.
    #[serde(default)]
    pub approval_id: Option<ApprovalId>,
}

/// The response to requests that return one skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillResult {
    /// The skill after the change.
    pub skill: Skill,
}

/// `skill.update`. Fields that are left out do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillUpdate {
    /// The skill.
    pub skill_id: SkillId,
    /// Turn it on or off. A blocked skill cannot be turned on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The new scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_in: Option<UseIn>,
    /// True moves the pin to the version in `update`. A new version that
    /// adds or changes a script turns the skill off until the user turns it
    /// on again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accept_new_version: bool,
}

/// `skill.remove`: delete a skill. Running sessions keep their copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRemove {
    /// The skill.
    pub skill_id: SkillId,
}

/// Which version of a skill to read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillVersion {
    /// The pinned version.
    #[default]
    Installed,
    /// The new version that waits for review.
    Pending,
}

/// `skill.files`: the file list of a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFiles {
    /// The skill.
    pub skill_id: SkillId,
    /// The version. The default is the pinned one.
    #[serde(default)]
    pub version: SkillVersion,
}

/// The response to `skill.files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFilesResult {
    /// The content hash of that version.
    pub content_hash: Sha256Hash,
    /// Every file, sorted by path.
    pub files: Vec<SkillFileInfo>,
}

/// `skill.read_file`: the text of one file of a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReadFile {
    /// The skill.
    pub skill_id: SkillId,
    /// The path in the skill folder.
    pub path: String,
    /// The version. The default is the pinned one.
    #[serde(default)]
    pub version: SkillVersion,
}

/// The response to `skill.read_file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFileContent {
    /// The path.
    pub path: String,
    /// The hash of the whole file.
    pub sha256: Sha256Hash,
    /// The size of the whole file in bytes.
    pub size: u64,
    /// The text, at most [`crate::model::SKILL_READ_MAX_BYTES`]. Empty for
    /// a file that is not UTF-8 text.
    #[serde(default)]
    pub content: Option<String>,
    /// True when the file is not UTF-8 text. Download it with
    /// `GET /v1/blobs/{sha256}`.
    #[serde(default)]
    pub binary: bool,
    /// True when `content` is cut.
    #[serde(default)]
    pub truncated: bool,
}
