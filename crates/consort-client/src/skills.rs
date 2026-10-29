//! Skills in the view state: the installed skills, the skill directories,
//! the preview before an install, and the file viewer.

use crate::connectors::{LoadState, UseInView};

/// The kind of a skill directory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DirectoryKind {
    #[default]
    Smithery,
    Anthropic,
    Marketplace,
    SkillsSh,
}

impl DirectoryKind {
    pub fn label(self) -> &'static str {
        match self {
            DirectoryKind::Smithery => "Smithery",
            DirectoryKind::Anthropic => "Anthropic",
            DirectoryKind::Marketplace => "Plugin marketplace",
            DirectoryKind::SkillsSh => "skills.sh",
        }
    }
}

/// Where a skill comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SkillSourceView {
    Git {
        url: String,
        path: Option<String>,
        git_ref: Option<String>,
    },
    Directory {
        directory: DirectoryKind,
        entry_id: String,
        url: String,
        path: Option<String>,
    },
    Upload {
        file_name: String,
    },
}

impl Default for SkillSourceView {
    fn default() -> Self {
        SkillSourceView::Upload {
            file_name: String::new(),
        }
    }
}

impl SkillSourceView {
    /// The source in a few words: "anthropics/skills" for a GitHub repo,
    /// the host and path of another URL, or the uploaded file name.
    pub fn short(&self) -> String {
        match self {
            SkillSourceView::Git { url, .. } | SkillSourceView::Directory { url, .. } => {
                repo_name(url)
            }
            SkillSourceView::Upload { file_name } => format!("Uploaded {file_name}"),
        }
    }

    /// The source as a link text, such as "github.com/anthropics/skills".
    pub fn location(&self) -> String {
        match self {
            SkillSourceView::Git { url, .. } | SkillSourceView::Directory { url, .. } => {
                let trimmed = url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches(".git")
                    .trim_end_matches('/');
                trimmed.to_owned()
            }
            SkillSourceView::Upload { file_name } => file_name.clone(),
        }
    }

    /// A web page for the source, when it has one.
    pub fn url(&self) -> Option<&str> {
        match self {
            SkillSourceView::Git { url, .. } | SkillSourceView::Directory { url, .. } => {
                Some(url.as_str()).filter(|u| u.starts_with("https://"))
            }
            SkillSourceView::Upload { .. } => None,
        }
    }

    pub fn can_update(&self) -> bool {
        !matches!(self, SkillSourceView::Upload { .. })
    }
}

/// "owner/repo" of a GitHub URL, or the URL without its scheme.
pub fn repo_name(url: &str) -> String {
    let trimmed = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches(".git")
        .trim_end_matches('/');
    match trimmed.strip_prefix("github.com/") {
        Some(rest) => rest.split('/').take(2).collect::<Vec<_>>().join("/"),
        None => trimmed.to_owned(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AuditStatusView {
    Passed,
    Warning,
    Failed,
    #[default]
    Unknown,
}

/// One audit result from a directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AuditView {
    /// Who audited it, such as "Socket".
    pub source: String,
    pub status: AuditStatusView,
    pub summary: Option<String>,
    pub url: Option<String>,
}

/// The audits of a skill in a few words, with the status that colors them.
pub fn audit_summary(audits: &[AuditView]) -> (AuditStatusView, String) {
    if audits.is_empty() {
        return (AuditStatusView::Unknown, "Not audited".to_owned());
    }
    let count = |s: AuditStatusView| audits.iter().filter(|a| a.status == s).count();
    let (failed, warnings, passed) = (
        count(AuditStatusView::Failed),
        count(AuditStatusView::Warning),
        count(AuditStatusView::Passed),
    );
    if failed > 0 {
        (AuditStatusView::Failed, "Audit failed".to_owned())
    } else if warnings > 0 {
        (
            AuditStatusView::Warning,
            if warnings == 1 {
                "1 warning".to_owned()
            } else {
                format!("{warnings} warnings")
            },
        )
    } else if passed == audits.len() {
        (AuditStatusView::Passed, format!("{passed} passed"))
    } else {
        (AuditStatusView::Unknown, "Not audited".to_owned())
    }
}

/// What changed in a file of a new version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FileChangeKind {
    #[default]
    Added,
    Modified,
    Removed,
}

/// Whether a skill has a new version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SkillUpdateView {
    #[default]
    UpToDate,
    Available {
        commit: Option<String>,
        changes: Vec<(String, FileChangeKind)>,
        scripts_changed: bool,
    },
    CheckFailed {
        message: String,
    },
}

/// An installed skill.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSourceView,
    /// The pinned commit, cut to 7 characters.
    pub commit: Option<String>,
    pub license: Option<String>,
    /// The files that can run.
    pub scripts: Vec<String>,
    pub audits: Vec<AuditView>,
    pub enabled: bool,
    pub use_in: UseInView,
    pub update: SkillUpdateView,
    /// Why the hub blocks it.
    pub blocked_reason: Option<String>,
    /// The hub took Claude-specific hooks out.
    pub hooks_removed: bool,
    pub file_count: u32,
    pub size_bytes: u64,
    /// When the skill was installed, such as "Oct 29".
    pub installed_label: String,
}

impl SkillView {
    pub fn has_scripts(&self) -> bool {
        !self.scripts.is_empty()
    }

    /// The state that the dot in the list shows.
    pub fn state(&self) -> SkillState {
        if self.blocked_reason.is_some() {
            SkillState::Blocked
        } else if matches!(self.update, SkillUpdateView::Available { .. }) {
            SkillState::NewVersion
        } else if self.enabled {
            SkillState::On
        } else {
            SkillState::Off
        }
    }
}

/// The state of a skill in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillState {
    On,
    Off,
    NewVersion,
    Blocked,
}

/// The line under a skill in the Settings list: "anthropics/skills, every
/// channel", "New version to review" or "Off. Has scripts, waiting for your
/// OK".
pub fn skill_row_detail(skill: &SkillView, channels: &[crate::view::ChannelView]) -> String {
    match skill.state() {
        SkillState::Blocked => "Blocked".to_owned(),
        SkillState::NewVersion => "New version to review".to_owned(),
        SkillState::Off if skill.has_scripts() => {
            "Off. Has scripts, waiting for your OK".to_owned()
        }
        SkillState::Off => "Off".to_owned(),
        SkillState::On => {
            let source = match &skill.source {
                SkillSourceView::Git { .. } => "Your git repo".to_owned(),
                other => other.short(),
            };
            format!(
                "{source}, {}",
                crate::connectors::use_in_short(&skill.use_in, channels)
            )
        }
    }
}

/// The "Skills in this channel" block that a session in the channel starts
/// with, as the hub renders it for a harness that loads skills itself.
pub fn skills_context(state: &crate::view::ViewState, channel_id: &str) -> String {
    let Some(channel) = state.channel(channel_id) else {
        return String::new();
    };
    let entries: Vec<consort_proto::model::SkillContextEntry> = state
        .skills
        .iter()
        .filter(|s| s.enabled && s.blocked_reason.is_none())
        .filter(|s| crate::connectors::use_in_matches(&s.use_in, channel))
        .filter(|s| !channel.disabled_skills.contains(&s.id))
        .map(|s| consort_proto::model::SkillContextEntry {
            name: s.name.clone(),
            description: s.description.clone(),
        })
        .collect();
    let block = consort_proto::model::render_skills_block(&entries, true);
    // The last line is for the session, not for the reader of Settings.
    block
        .lines()
        .filter(|l| !l.starts_with("They are installed"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A skill directory that the hub searches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectorySourceView {
    pub id: String,
    pub kind: DirectoryKind,
    pub name: String,
    pub url: String,
    pub removable: bool,
}

/// One result of a directory search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectoryEntryView {
    pub entry_id: String,
    pub directory: DirectoryKind,
    /// The source to name when the entry is installed.
    pub source_id: Option<String>,
    pub name: String,
    pub description: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub license: Option<String>,
    pub installs: Option<u64>,
    pub audits: Vec<AuditView>,
    /// The installed skill, when the user has this one.
    pub installed_skill_id: Option<String>,
}

/// The skill directory search of the Add skill dialog, paged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillDirectoryView {
    pub search: String,
    pub source: Option<String>,
    pub sources: Vec<DirectorySourceView>,
    pub entries: Vec<DirectoryEntryView>,
    pub next_cursor: Option<String>,
    /// The directories that did not answer.
    pub unavailable: Vec<String>,
    pub load: LoadState,
    pub more: LoadState,
}

/// Where the Add skill dialog installs from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillInstallInput {
    Directory {
        source: Option<String>,
        entry_id: String,
    },
    Git {
        url: String,
        path: Option<String>,
        git_ref: Option<String>,
    },
    /// A zip file on this machine.
    Upload { file_path: String },
}

/// What an install would bring, before it happens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillPreviewView {
    pub name: String,
    pub description: String,
    pub commit: Option<String>,
    pub license: Option<String>,
    pub scripts: Vec<String>,
    pub audits: Vec<AuditView>,
    pub file_count: u32,
    pub has_hooks: bool,
    pub blocked_reason: Option<String>,
    pub installed_skill_id: Option<String>,
}

/// One file of a skill.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillFileView {
    pub path: String,
    pub size: u64,
    pub executable: bool,
}

/// The text of one file of a skill.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillFileContentView {
    pub path: String,
    pub content: Option<String>,
    pub binary: bool,
    pub truncated: bool,
}

/// The first 7 characters of a commit.
pub fn short_commit(commit: &str) -> String {
    commit.chars().take(7).collect()
}

/// The source line of a skill page: "github.com/anthropics/skills at
/// 3f2a91c. Apache-2.0. 4 files."
pub fn source_line(skill: &SkillView) -> String {
    let mut parts = [skill.source.location()];
    if let Some(commit) = &skill.commit {
        parts[0] = format!("{} at {commit}", parts[0]);
    }
    let mut out = format!("{}.", parts[0]);
    if let Some(license) = skill.license.as_deref().filter(|l| !l.is_empty()) {
        out.push_str(&format!(" {license}."));
    }
    out.push_str(&match skill.file_count {
        1 => " 1 file.".to_owned(),
        n => format!(" {n} files."),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit(status: AuditStatusView) -> AuditView {
        AuditView {
            source: "Socket".into(),
            status,
            ..AuditView::default()
        }
    }

    #[test]
    fn audits_read_in_a_few_words() {
        assert_eq!(audit_summary(&[]).1, "Not audited");
        assert_eq!(
            audit_summary(&[
                audit(AuditStatusView::Passed),
                audit(AuditStatusView::Passed)
            ]),
            (AuditStatusView::Passed, "2 passed".to_owned())
        );
        assert_eq!(
            audit_summary(&[
                audit(AuditStatusView::Passed),
                audit(AuditStatusView::Warning)
            ]),
            (AuditStatusView::Warning, "1 warning".to_owned())
        );
        assert_eq!(
            audit_summary(&[
                audit(AuditStatusView::Warning),
                audit(AuditStatusView::Failed)
            ])
            .0,
            AuditStatusView::Failed
        );
    }

    #[test]
    fn a_source_reads_as_its_repo() {
        assert_eq!(
            repo_name("https://github.com/anthropics/skills.git"),
            "anthropics/skills"
        );
        assert_eq!(
            repo_name("https://github.com/anthropics/skills/tree/main"),
            "anthropics/skills"
        );
        assert_eq!(
            repo_name("https://git.example.com/me/x"),
            "git.example.com/me/x"
        );
        let skill = SkillView {
            source: SkillSourceView::Git {
                url: "https://github.com/anthropics/skills".into(),
                path: Some("deep-research".into()),
                git_ref: None,
            },
            commit: Some("3f2a91c".into()),
            license: Some("Apache-2.0".into()),
            file_count: 4,
            ..SkillView::default()
        };
        assert_eq!(
            source_line(&skill),
            "github.com/anthropics/skills at 3f2a91c. Apache-2.0. 4 files."
        );
    }

    #[test]
    fn the_list_line_follows_the_state() {
        let mut skill = SkillView {
            enabled: true,
            source: SkillSourceView::Directory {
                directory: DirectoryKind::Anthropic,
                entry_id: "pdf".into(),
                url: "https://github.com/anthropics/skills".into(),
                path: None,
            },
            ..SkillView::default()
        };
        assert_eq!(
            skill_row_detail(&skill, &[]),
            "anthropics/skills, every channel"
        );
        skill.enabled = false;
        skill.scripts = vec!["scripts/run.sh".into()];
        assert_eq!(
            skill_row_detail(&skill, &[]),
            "Off. Has scripts, waiting for your OK"
        );
        skill.update = SkillUpdateView::Available {
            commit: None,
            changes: Vec::new(),
            scripts_changed: false,
        };
        assert_eq!(skill.state(), SkillState::NewVersion);
        assert_eq!(skill_row_detail(&skill, &[]), "New version to review");
    }
}
