//! Skills: folders in the Agent Skills format that the hub keeps and the
//! workers deliver to sessions.
//!
//! The hub installs a skill from a source (a git repo plus a path, a
//! directory entry, or an upload), pins it to a commit and a content hash,
//! and stores its files content-addressed. A session gets the skills of its
//! channel's effective set. The worker fetches each bundle by its content
//! hash (`skill_bundle.get`) and keeps it in a cache.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::common::{Sha256Hash, ValidationError, validate_relative_path};
use super::connector::{UseIn, one_line};
use super::conversation::Channel;
use crate::ids::SkillId;
use crate::time::Timestamp;

/// The most characters in a skill name (the Agent Skills limit).
pub const SKILL_NAME_MAX_CHARS: usize = 64;
/// The most characters in a skill description (the Agent Skills limit).
pub const SKILL_DESCRIPTION_MAX_CHARS: usize = 1_024;
/// The most files in one skill.
pub const SKILL_MAX_FILES: usize = 500;
/// The most bytes of all files of one skill together (20 MiB).
pub const SKILL_MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
/// The most bytes of one file of a skill (5 MiB).
pub const SKILL_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// The most bytes of an uploaded zip file (25 MiB).
pub const SKILL_UPLOAD_MAX_BYTES: u64 = 25 * 1024 * 1024;
/// The most bytes of text that `skill.read_file` returns (512 KiB). A longer
/// file is cut, with `truncated` set.
pub const SKILL_READ_MAX_BYTES: usize = 512 * 1024;
/// The file that every skill has at its root.
pub const SKILL_MANIFEST_FILE: &str = "SKILL.md";
/// How often the hub checks the sources of installed skills for a new
/// version, in milliseconds (one day).
pub const SKILL_UPDATE_CHECK_MS: i64 = 24 * 60 * 60 * 1_000;

/// Checks a skill name: 1 to 64 lowercase ASCII letters, digits and `-`,
/// with no `-` at the start or the end and no `--`.
pub fn validate_skill_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() {
        return Err(ValidationError::new("name", "is empty"));
    }
    if name.chars().count() > SKILL_NAME_MAX_CHARS {
        return Err(ValidationError::new(
            "name",
            format!("is longer than {SKILL_NAME_MAX_CHARS} characters"),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ValidationError::new(
            "name",
            "may hold only lowercase letters, digits and -",
        ));
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(ValidationError::new(
            "name",
            "may not start or end with - or hold --",
        ));
    }
    Ok(())
}

/// Checks a skill description: not empty after a trim, at most 1,024
/// characters.
pub fn validate_skill_description(description: &str) -> Result<(), ValidationError> {
    if description.trim().is_empty() {
        return Err(ValidationError::new("description", "is empty"));
    }
    if description.chars().count() > SKILL_DESCRIPTION_MAX_CHARS {
        return Err(ValidationError::new(
            "description",
            format!("is longer than {SKILL_DESCRIPTION_MAX_CHARS} characters"),
        ));
    }
    Ok(())
}

/// Where a skill comes from. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillSource {
    /// A git repo, with the folder of the skill in it.
    Git {
        /// The clone URL, for example `https://github.com/anthropics/skills`.
        url: String,
        /// The folder of the skill in the repo. Empty for the repo root.
        #[serde(default)]
        path: Option<String>,
        /// The branch or tag that updates follow. Empty for the default
        /// branch.
        #[serde(default, rename = "ref")]
        git_ref: Option<String>,
    },
    /// An entry of a skill directory. The files still come from git.
    Directory {
        /// The directory.
        directory: SkillDirectoryKind,
        /// The entry ID in the directory.
        entry_id: String,
        /// The git repo of the entry.
        url: String,
        /// The folder of the skill in the repo.
        #[serde(default)]
        path: Option<String>,
    },
    /// A zip file that the user uploaded. It gets no updates.
    Upload {
        /// The file name, as the user gave it.
        file_name: String,
    },
}

impl SkillSource {
    /// Whether the hub can check this source for a new version.
    pub fn can_update(&self) -> bool {
        !matches!(self, Self::Upload { .. })
    }
}

/// The kind of a skill directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillDirectoryKind {
    /// The Smithery public registry.
    Smithery,
    /// Anthropic's skills repo.
    Anthropic,
    /// A Claude plugin `marketplace.json` file that the user added.
    Marketplace,
    /// skills.sh. Its API needs a Vercel token, so the app links to it and
    /// the user pastes an `owner/repo`.
    SkillsSh,
}

/// The result of one audit of a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    /// The audit found no problem.
    Passed,
    /// The audit found something to check.
    Warning,
    /// The audit failed. The hub blocks the skill.
    Failed,
    /// The audit gave no clear result.
    Unknown,
}

/// One audit result, as a directory gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillAudit {
    /// Who ran the audit, for example "Smithery".
    pub source: String,
    /// The result.
    pub status: AuditStatus,
    /// One line about the result.
    #[serde(default)]
    pub summary: Option<String>,
    /// The full report.
    #[serde(default)]
    pub url: Option<String>,
}

/// One file of a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFileInfo {
    /// The path in the skill folder, with `/` between parts.
    pub path: String,
    /// The SHA-256 of the content. The content is a blob on the hub.
    pub sha256: Sha256Hash,
    /// The size in bytes.
    pub size: u64,
    /// Whether the file has the executable bit.
    #[serde(default)]
    pub executable: bool,
}

/// How a file changes in a new version of a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillFileChangeKind {
    /// The file is new.
    Added,
    /// The content changed.
    Modified,
    /// The file is gone.
    Removed,
}

/// One changed file in a new version of a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFileChange {
    /// The path.
    pub path: String,
    /// How it changes.
    pub change: SkillFileChangeKind,
}

/// Whether a skill has a new version. Serialized with a `kind` field.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillUpdateState {
    /// The installed version is the newest one that the hub knows, or the
    /// source cannot update.
    #[default]
    UpToDate,
    /// "New version to review". Only the user's approval moves the pin.
    /// Running sessions keep their version.
    Available {
        /// The new commit.
        commit: Option<String>,
        /// The content hash of the new version.
        content_hash: Sha256Hash,
        /// The files that change.
        #[serde(default)]
        changes: Vec<SkillFileChange>,
        /// Whether the new version adds or changes a script.
        #[serde(default)]
        scripts_changed: bool,
        /// When the hub found it.
        found_at: Timestamp,
    },
    /// The last check failed.
    CheckFailed {
        /// What went wrong.
        message: String,
    },
}

/// An installed skill. A feed entity with no channel (`skill`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// The ID.
    pub id: SkillId,
    /// The `name` of its `SKILL.md`. Unique on the hub.
    pub name: String,
    /// The `description` of its `SKILL.md`.
    pub description: String,
    /// Where it comes from.
    pub source: SkillSource,
    /// The pinned commit. Empty for an upload.
    pub commit: Option<String>,
    /// The hash of the pinned files. See [`skill_content_hash`].
    pub content_hash: Sha256Hash,
    /// The license, from `SKILL.md` or a license file.
    #[serde(default)]
    pub license: Option<String>,
    /// The paths of the files that can run: files with the executable bit,
    /// and files under `scripts/`.
    #[serde(default)]
    pub scripts: Vec<String>,
    /// The audit results that the directory gave.
    #[serde(default)]
    pub audits: Vec<SkillAudit>,
    /// Whether sessions get it. A skill with scripts stays off until the
    /// user turns it on.
    pub enabled: bool,
    /// Where it can be used.
    pub use_in: UseIn,
    /// Whether a new version waits for review.
    #[serde(default)]
    pub update: SkillUpdateState,
    /// Why the hub blocks it, for example a failed audit. A blocked skill
    /// cannot be turned on.
    #[serde(default)]
    pub blocked_reason: Option<String>,
    /// True when the hub removed Claude-specific `hooks` from it.
    #[serde(default)]
    pub hooks_removed: bool,
    /// The number of files.
    #[serde(default)]
    pub file_count: u32,
    /// The size of all files in bytes.
    #[serde(default)]
    pub size_bytes: u64,
    /// When it was installed.
    pub installed_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
}

impl Skill {
    /// Whether it has files that can run.
    pub fn has_scripts(&self) -> bool {
        !self.scripts.is_empty()
    }

    /// Whether a session in `channel` gets this skill: it is on, not
    /// blocked, its scope matches, and the channel did not turn it off.
    pub fn is_available_in(&self, channel: &Channel) -> bool {
        self.enabled
            && self.blocked_reason.is_none()
            && self.use_in.matches_channel(channel)
            && !channel.disabled_skills.contains(&self.id)
    }

    /// The reference that `session.start` carries.
    pub fn bundle_ref(&self) -> SkillBundleRef {
        SkillBundleRef {
            skill_id: self.id,
            name: self.name.clone(),
            content_hash: self.content_hash.clone(),
        }
    }
}

/// A skill directory that the hub searches. The Smithery registry and
/// Anthropic's skills repo are always there. The user adds marketplace
/// files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDirectorySource {
    /// The ID: `smithery`, `anthropic`, or the URL of a marketplace file.
    pub id: String,
    /// The kind.
    pub kind: SkillDirectoryKind,
    /// The name to show.
    pub name: String,
    /// The URL.
    pub url: String,
    /// False for the built-in directories.
    #[serde(default)]
    pub removable: bool,
}

/// One search result from a skill directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDirectoryEntry {
    /// The entry ID in its directory. `skill.install` takes it.
    pub entry_id: String,
    /// The directory.
    pub directory: SkillDirectoryKind,
    /// The ID of the directory source, for a marketplace.
    #[serde(default)]
    pub source_id: Option<String>,
    /// The skill name.
    pub name: String,
    /// What it does.
    #[serde(default)]
    pub description: String,
    /// The author or the owner of the repo.
    #[serde(default)]
    pub author: Option<String>,
    /// The page of the entry.
    #[serde(default)]
    pub url: Option<String>,
    /// The git repo.
    #[serde(default)]
    pub repo_url: Option<String>,
    /// The folder of the skill in the repo.
    #[serde(default)]
    pub path: Option<String>,
    /// The license, when the directory gives it.
    #[serde(default)]
    pub license: Option<String>,
    /// The install count, when the directory gives it.
    #[serde(default)]
    pub installs: Option<u64>,
    /// The audit results, when the directory gives them.
    #[serde(default)]
    pub audits: Vec<SkillAudit>,
    /// The installed skill from this entry, when there is one.
    #[serde(default)]
    pub installed_skill_id: Option<SkillId>,
}

/// A skill that a session gets, as `session.start` names it. The worker
/// fetches the files with `skill_bundle.get` when its cache does not have
/// this content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBundleRef {
    /// The skill.
    pub skill_id: SkillId,
    /// The skill name. The worker writes the skill to a folder with this
    /// name.
    pub name: String,
    /// The hash of the files.
    pub content_hash: Sha256Hash,
}

/// The files of one version of a skill: the result of `skill_bundle.get`.
/// The worker downloads each file with `GET /v1/blobs/{sha256}` and checks
/// [`skill_content_hash`] before it uses the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBundle {
    /// The skill.
    pub skill_id: SkillId,
    /// The skill name.
    pub name: String,
    /// The hash of the files.
    pub content_hash: Sha256Hash,
    /// Every file, sorted by path.
    pub files: Vec<SkillFileInfo>,
}

impl SkillBundle {
    /// Whether the files give the content hash.
    pub fn is_consistent(&self) -> bool {
        skill_content_hash(&self.files) == self.content_hash
    }
}

/// The content hash of a skill version: the SHA-256 of one line per file,
/// sorted by path, each `<path>\0<sha256>\0<x or ->\n`, where `x` marks an
/// executable file. The hub and the worker compute it the same way.
pub fn skill_content_hash(files: &[SkillFileInfo]) -> Sha256Hash {
    let mut sorted: Vec<&SkillFileInfo> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hasher = Sha256::new();
    for file in sorted {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.sha256.as_str().as_bytes());
        hasher.update([0]);
        hasher.update(if file.executable { b"x" } else { b"-" });
        hasher.update(b"\n");
    }
    Sha256Hash::from_bytes(hasher.finalize().into())
}

/// Checks the file list of a skill: a `SKILL.md` at the root, safe relative
/// paths, no repeated path, and the count and size limits. The hub refuses
/// symlinks and absolute paths before it builds the list.
pub fn validate_skill_files(files: &[SkillFileInfo]) -> Result<(), ValidationError> {
    if files.len() > SKILL_MAX_FILES {
        return Err(ValidationError::new(
            "files",
            format!("has more than {SKILL_MAX_FILES} files"),
        ));
    }
    let mut total: u64 = 0;
    let mut paths = std::collections::BTreeSet::new();
    for file in files {
        validate_relative_path("files.path", &file.path)?;
        if !paths.insert(file.path.as_str()) {
            return Err(ValidationError::new(
                "files.path",
                format!("{} is there twice", file.path),
            ));
        }
        if file.size > SKILL_MAX_FILE_BYTES {
            return Err(ValidationError::new(
                "files.size",
                format!("{} is larger than {SKILL_MAX_FILE_BYTES} bytes", file.path),
            ));
        }
        total = total.saturating_add(file.size);
    }
    if total > SKILL_MAX_TOTAL_BYTES {
        return Err(ValidationError::new(
            "files",
            format!("are larger than {SKILL_MAX_TOTAL_BYTES} bytes together"),
        ));
    }
    if !paths.contains(SKILL_MANIFEST_FILE) {
        return Err(ValidationError::new(
            "files",
            format!("has no {SKILL_MANIFEST_FILE} at the root"),
        ));
    }
    Ok(())
}

/// One skill in the "Skills in this channel" context block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillContextEntry {
    /// The skill name.
    pub name: String,
    /// What it does.
    pub description: String,
}

impl From<&Skill> for SkillContextEntry {
    fn from(skill: &Skill) -> Self {
        Self {
            name: skill.name.clone(),
            description: skill.description.clone(),
        }
    }
}

impl fmt::Display for SkillContextEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}",
            one_line(&self.name),
            one_line(&self.description)
        )
    }
}

/// The "Skills in this channel" block for a session's prompt. Empty when the
/// channel has no skill.
///
/// When the harness loads skills itself (`native` is true), the block only
/// names them. Otherwise it tells the session to read a skill with the
/// `list_skills` and `read_skill` hub tools.
pub fn render_skills_block(entries: &[SkillContextEntry], native: bool) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::from("Skills in this channel:\n");
    for entry in entries {
        out.push_str("- ");
        out.push_str(&entry.to_string());
        out.push('\n');
    }
    if native {
        out.push_str("They are installed as skills. Use one when the task fits it.");
    } else {
        out.push_str("Before you use one, call read_skill with its name and follow its SKILL.md.");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(path: &str, fill: char, executable: bool) -> SkillFileInfo {
        SkillFileInfo {
            path: path.into(),
            sha256: fill.to_string().repeat(64).parse().unwrap(),
            size: 10,
            executable,
        }
    }

    pub(crate) fn skill() -> Skill {
        let files = vec![
            file("SKILL.md", 'a', false),
            file("scripts/fill.py", 'b', true),
        ];
        Skill {
            id: "skl_00000000-0000-0000-0000-000000000001".parse().unwrap(),
            name: "pdf-forms".into(),
            description: "Fill and read PDF forms.".into(),
            source: SkillSource::Git {
                url: "https://github.com/anthropics/skills".into(),
                path: Some("document-skills/pdf".into()),
                git_ref: None,
            },
            commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
            content_hash: skill_content_hash(&files),
            license: Some("Apache-2.0".into()),
            scripts: vec!["scripts/fill.py".into()],
            audits: vec![SkillAudit {
                source: "Smithery".into(),
                status: AuditStatus::Passed,
                summary: None,
                url: None,
            }],
            enabled: false,
            use_in: UseIn::EveryChannel,
            update: SkillUpdateState::UpToDate,
            blocked_reason: None,
            hooks_removed: false,
            file_count: 2,
            size_bytes: 20,
            installed_at: Timestamp::from_millis(1),
            updated_at: Timestamp::from_millis(2),
        }
    }

    #[test]
    fn names_follow_the_agent_skills_rules() {
        for good in ["pdf", "pdf-forms", "a1", &"a".repeat(64)] {
            assert!(validate_skill_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "PDF",
            "pdf_forms",
            "-pdf",
            "pdf-",
            "pdf--forms",
            &"a".repeat(65),
            "pdf forms",
        ] {
            assert!(validate_skill_name(bad).is_err(), "{bad}");
        }
        assert!(validate_skill_description("Fill PDF forms.").is_ok());
        assert!(validate_skill_description(" ").is_err());
        assert!(validate_skill_description(&"d".repeat(1_025)).is_err());
    }

    #[test]
    fn skill_round_trips_with_each_source_and_update_state() {
        let mut s = skill();
        let sources = [
            s.source.clone(),
            SkillSource::Directory {
                directory: SkillDirectoryKind::Smithery,
                entry_id: "@anthropic/pdf".into(),
                url: "https://github.com/anthropics/skills".into(),
                path: Some("pdf".into()),
            },
            SkillSource::Upload {
                file_name: "my-skill.zip".into(),
            },
        ];
        let updates = [
            SkillUpdateState::UpToDate,
            SkillUpdateState::Available {
                commit: Some("fedcba".into()),
                content_hash: "c".repeat(64).parse().unwrap(),
                changes: vec![SkillFileChange {
                    path: "SKILL.md".into(),
                    change: SkillFileChangeKind::Modified,
                }],
                scripts_changed: false,
                found_at: Timestamp::from_millis(5),
            },
            SkillUpdateState::CheckFailed {
                message: "The repo is gone.".into(),
            },
        ];
        for (source, update) in sources.into_iter().zip(updates) {
            s.source = source;
            s.update = update;
            let value = serde_json::to_value(&s).unwrap();
            assert!(value["source"]["kind"].is_string());
            assert!(value["update"]["kind"].is_string());
            assert_eq!(serde_json::from_value::<Skill>(value).unwrap(), s);
        }
        let git = serde_json::to_value(&skill().source).unwrap();
        assert_eq!(
            git,
            json!({"kind": "git", "url": "https://github.com/anthropics/skills", "path": "document-skills/pdf", "ref": null})
        );
        assert!(
            !SkillSource::Upload {
                file_name: "a.zip".into()
            }
            .can_update()
        );
        // A skill from an older hub has no update state.
        let mut value = serde_json::to_value(skill()).unwrap();
        value.as_object_mut().unwrap().remove("update");
        assert_eq!(
            serde_json::from_value::<Skill>(value).unwrap().update,
            SkillUpdateState::UpToDate
        );
    }

    #[test]
    fn content_hash_is_order_free_and_sees_every_field() {
        let a = file("SKILL.md", 'a', false);
        let b = file("scripts/run.sh", 'b', true);
        let one = skill_content_hash(&[a.clone(), b.clone()]);
        assert_eq!(one, skill_content_hash(&[b.clone(), a.clone()]));
        let mut not_exec = b.clone();
        not_exec.executable = false;
        assert_ne!(one, skill_content_hash(&[a.clone(), not_exec]));
        let mut renamed = b.clone();
        renamed.path = "scripts/run2.sh".into();
        assert_ne!(one, skill_content_hash(&[a.clone(), renamed]));
        // The exact bytes are fixed, so the hub and the worker agree.
        assert_eq!(
            skill_content_hash(&[]).as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let bundle = SkillBundle {
            skill_id: skill().id,
            name: "pdf-forms".into(),
            content_hash: one.clone(),
            files: vec![a, b],
        };
        assert!(bundle.is_consistent());
        let back: SkillBundle =
            serde_json::from_value(serde_json::to_value(&bundle).unwrap()).unwrap();
        assert_eq!(back, bundle);
        let mut wrong = bundle;
        wrong.files.pop();
        assert!(!wrong.is_consistent());
    }

    #[test]
    fn file_lists_are_checked() {
        let ok = vec![
            file("SKILL.md", 'a', false),
            file("ref/notes.md", 'b', false),
        ];
        assert!(validate_skill_files(&ok).is_ok());
        assert!(validate_skill_files(&ok[1..]).is_err());
        for bad in ["../x", "/etc/passwd", "a//b", "a\\b"] {
            let mut files = ok.clone();
            files.push(file(bad, 'c', false));
            assert!(validate_skill_files(&files).is_err(), "{bad}");
        }
        let mut twice = ok.clone();
        twice.push(file("SKILL.md", 'c', false));
        assert!(validate_skill_files(&twice).is_err());
        let mut big = ok.clone();
        big[1].size = SKILL_MAX_FILE_BYTES + 1;
        assert!(validate_skill_files(&big).is_err());
    }

    #[test]
    fn directory_types_round_trip() {
        let entry = SkillDirectoryEntry {
            entry_id: "@anthropic/pdf".into(),
            directory: SkillDirectoryKind::Smithery,
            source_id: None,
            name: "pdf".into(),
            description: "PDF tools.".into(),
            author: Some("anthropic".into()),
            url: Some("https://smithery.ai/skills/@anthropic/pdf".into()),
            repo_url: Some("https://github.com/anthropics/skills".into()),
            path: Some("pdf".into()),
            license: None,
            installs: Some(1_200),
            audits: vec![SkillAudit {
                source: "Smithery".into(),
                status: AuditStatus::Warning,
                summary: Some("Runs a Python script.".into()),
                url: None,
            }],
            installed_skill_id: None,
        };
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["directory"], "smithery");
        assert_eq!(
            serde_json::from_value::<SkillDirectoryEntry>(value).unwrap(),
            entry
        );
        let source = SkillDirectorySource {
            id: "https://example.com/.claude-plugin/marketplace.json".into(),
            kind: SkillDirectoryKind::Marketplace,
            name: "Example marketplace".into(),
            url: "https://example.com/.claude-plugin/marketplace.json".into(),
            removable: true,
        };
        assert_eq!(
            serde_json::from_value::<SkillDirectorySource>(serde_json::to_value(&source).unwrap())
                .unwrap(),
            source
        );
        assert_eq!(
            serde_json::to_value(SkillDirectoryKind::SkillsSh).unwrap(),
            "skills_sh"
        );
    }

    #[test]
    fn skills_block() {
        assert_eq!(render_skills_block(&[], true), "");
        let entries = [SkillContextEntry::from(&skill())];
        assert_eq!(
            render_skills_block(&entries, true),
            "Skills in this channel:\n- pdf-forms: Fill and read PDF forms.\nThey are installed as skills. Use one when the task fits it."
        );
        assert!(render_skills_block(&entries, false).ends_with(
            "Before you use one, call read_skill with its name and follow its SKILL.md."
        ));
    }

    #[test]
    fn bundle_ref_comes_from_the_skill() {
        let s = skill();
        let r = s.bundle_ref();
        assert_eq!(r.skill_id, s.id);
        assert_eq!(r.content_hash, s.content_hash);
        assert!(s.has_scripts());
        let back: SkillBundleRef =
            serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}
