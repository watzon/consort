//! Skills: installed skills, their files as blobs, the client requests
//! that read and change them, `skill_bundle.get` for workers, and the
//! `list_skills` and `read_skill` hub tools.
//!
//! The install pipeline (step 3 of docs/CONNECTORS-SKILLS-PLAN.md) lives in
//! the submodules:
//! - [`directory`]: the skill directories, their cache and search.
//! - [`git`]: fetches a skill folder at a commit with the `git` CLI.
//! - [`upload`]: reads an uploaded zip in memory.
//! - [`package`]: the checks that every source goes through.
//! - [`install`]: `skill.preview`, `skill.install`, the install card, and
//!   the `search_skills` and `add_skill` tools.
//! - [`updates`]: the daily check for a new version.

pub(crate) mod directory;
pub(crate) mod git;
pub(crate) mod install;
pub(crate) mod package;
pub(crate) mod remote;
mod tarball;
pub(crate) mod updates;
mod upload;

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod flow_tests;

#[cfg(test)]
mod remote_tests;

pub use directory::SkillSources;
pub use updates::UpdateReport;

use consort_proto::client_api::{
    SkillFileContent, SkillFiles, SkillFilesResult, SkillReadFile, SkillRemove, SkillResult,
    SkillUpdate, SkillVersion,
};
use consort_proto::error::ErrorCode;
use consort_proto::frame::Empty;
use consort_proto::ids::{SkillId, WorkerId};
use consort_proto::model::{
    SKILL_MANIFEST_FILE, SKILL_READ_MAX_BYTES, Sha256Hash, Skill, SkillAudit, SkillBundle,
    SkillContextEntry, SkillFileInfo, SkillSource, SkillUpdateState, UseIn,
};
use consort_proto::tools::{ListSkillsResult, ReadSkillArgs, ReadSkillResult};
use consort_proto::worker_link::SkillBundleGet;
use serde_json::Value;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::store::{conversation, meta_get, skills as skill_store};
use package::{Prepared, is_script};

/// The harnesses that load skills from a folder themselves (see the
/// delivery list in docs/CONNECTORS-SKILLS-PLAN.md). Every other harness
/// gets `list_skills` and `read_skill`.
const NATIVE_SKILL_HARNESSES: &[&str] = &["claude", "codex", "gemini", "opencode"];

/// Whether a harness loads skills itself. `harness` is the account's
/// harness name, such as `claude` or `acp`.
pub(crate) fn harness_loads_skills(harness: &str) -> bool {
    NATIVE_SKILL_HARNESSES.contains(&harness)
}

/// One file of a skill to store.
#[derive(Debug, Clone)]
pub(crate) struct NewSkillFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}

/// A skill to check and store.
#[derive(Debug, Clone)]
pub(crate) struct NewSkill {
    pub source: SkillSource,
    pub commit: Option<String>,
    pub files: Vec<NewSkillFile>,
    pub use_in: UseIn,
    /// `None` turns on a skill with no scripts, and leaves one with
    /// scripts off.
    pub enabled: Option<bool>,
    pub license: Option<String>,
    pub audits: Vec<SkillAudit>,
}

/// The facts of a skill to store that do not come from its files.
#[derive(Debug, Clone)]
pub(crate) struct StoreMeta {
    pub source: SkillSource,
    pub commit: Option<String>,
    pub use_in: UseIn,
    /// `None` turns on a skill with no scripts, and leaves one with
    /// scripts off.
    pub enabled: Option<bool>,
    pub audits: Vec<SkillAudit>,
    /// A license that wins over the one of the files.
    pub license: Option<String>,
}

/// What a new version changes in the facts of a skill.
struct PendingFacts {
    content_hash: Sha256Hash,
    description: Option<String>,
    license: Option<String>,
    hooks_removed: bool,
}

/// The `name`, `description` and `license` of a `SKILL.md` front matter.
/// It reads plain, quoted and folded (`>` or `|`) values.
pub(crate) fn front_matter(text: &str) -> (Option<String>, Option<String>, Option<String>) {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, None, None);
    }
    let body: Vec<&str> = lines.take_while(|l| l.trim_end() != "---").collect();
    let value_of = |key: &str| -> Option<String> {
        let prefix = format!("{key}:");
        let start = body.iter().position(|l| l.starts_with(&prefix))?;
        let rest = body[start][prefix.len()..].trim();
        let value = if rest.starts_with('>') || rest.starts_with('|') {
            let keep_lines = rest.starts_with('|');
            let parts: Vec<&str> = body[start + 1..]
                .iter()
                .take_while(|l| l.starts_with(' ') || l.starts_with('\t') || l.trim().is_empty())
                .map(|l| l.trim())
                .collect();
            parts
                .join(if keep_lines { "\n" } else { " " })
                .trim()
                .to_owned()
        } else {
            let unquoted = rest
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                .unwrap_or(rest);
            unquoted.to_owned()
        };
        Some(value).filter(|v| !v.is_empty())
    };
    (
        value_of("name"),
        value_of("description"),
        value_of("license"),
    )
}

fn skill_blocked(message: impl Into<String>) -> HubError {
    HubError::new(ErrorCode::SkillBlocked, message)
}

impl Hub {
    /// Stores the bytes as a blob named by their SHA-256. A blob that is
    /// there already stays.
    pub(crate) async fn put_blob_bytes(&self, bytes: Vec<u8>) -> Result<Sha256Hash> {
        let hash = crate::util::sha256(&bytes);
        let target = self.store.blobs_dir().join(hash.as_str());
        if tokio::fs::try_exists(&target).await? {
            return Ok(hash);
        }
        let temp = target.with_extension(format!("tmp-{}", &crate::util::random_token()[..12]));
        let mut file = crate::util::create_private_file(&temp).await?;
        let written = async {
            use tokio::io::AsyncWriteExt;
            file.write_all(&bytes).await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&temp, &target).await
        }
        .await;
        if let Err(err) = written {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(err.into());
        }
        Ok(hash)
    }

    async fn read_blob_bytes(&self, hash: &Sha256Hash) -> Result<Vec<u8>> {
        let path = self.store.blobs_dir().join(hash.as_str());
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(HubError::not_found(
                "A file of this skill is missing on the hub. Remove the skill and add it again.",
            )),
            Err(err) => Err(err.into()),
        }
    }

    /// Checks and stores a skill: its files as blobs, its version, and the
    /// skill. The name comes from `SKILL.md` and must be unique.
    pub(crate) async fn store_skill(&self, new: NewSkill) -> Result<Skill> {
        let prepared = package::prepare(new.files, None)?;
        self.store_prepared(
            prepared,
            StoreMeta {
                source: new.source,
                commit: new.commit,
                use_in: new.use_in,
                enabled: new.enabled,
                audits: new.audits,
                license: new.license,
            },
        )
        .await
    }

    /// Stores a checked skill. A failed audit blocks it; a skill with
    /// scripts starts off unless `enabled` says otherwise.
    pub(crate) async fn store_prepared(
        &self,
        prepared: Prepared,
        meta: StoreMeta,
    ) -> Result<Skill> {
        let use_in = self.check_use_in(meta.use_in).await?;
        if let Some(existing) = self.installed_skill_named(&prepared.name).await? {
            return Err(HubError::conflict(format!(
                "A skill named {} is installed already. Remove it first, or update it.",
                existing.name
            )));
        }
        let Prepared {
            name,
            description,
            license,
            files,
            infos,
            content_hash,
            scripts,
            hooks_removed,
        } = prepared;
        for file in files {
            self.put_blob_bytes(file.bytes).await?;
        }
        let blocked_reason = install::blocked_reason(&meta.audits);
        let enabled = blocked_reason.is_none() && meta.enabled.unwrap_or(scripts.is_empty());
        let size_bytes = infos.iter().map(|f| f.size).sum();
        let file_count = u32::try_from(infos.len()).unwrap_or(u32::MAX);
        self.store
            .write(move |tx| {
                if skill_store::find_skill(tx.db(), &name)?.is_some() {
                    return Err(HubError::conflict(format!(
                        "A skill named {name} is installed already. Remove it first, or update it."
                    )));
                }
                let now = tx.now();
                let skill = Skill {
                    id: SkillId::new(),
                    name,
                    description,
                    source: meta.source,
                    commit: meta.commit,
                    content_hash: content_hash.clone(),
                    license: meta.license.or(license),
                    scripts,
                    audits: meta.audits,
                    enabled,
                    use_in,
                    update: SkillUpdateState::UpToDate,
                    blocked_reason,
                    hooks_removed,
                    file_count,
                    size_bytes,
                    installed_at: now,
                    updated_at: now,
                };
                tx.save_skill_version(skill.id, &skill.name, &content_hash, &infos)?;
                if hooks_removed {
                    tx.meta_set(&updates::hooks_meta_key(content_hash.as_str()), "1")?;
                }
                tx.save_skill(&skill)?;
                Ok(skill)
            })
            .await
    }

    async fn installed_skill_named(&self, name: &str) -> Result<Option<Skill>> {
        let name = name.to_owned();
        self.store
            .read(move |conn| skill_store::find_skill(conn, &name))
            .await
    }

    /// The `description`, the `license` and the hooks flag of the version
    /// that waits for review, read from its stored `SKILL.md`.
    async fn pending_facts(&self, id: SkillId) -> Result<Option<PendingFacts>> {
        let found = self
            .store
            .read(move |conn| {
                let skill = skill_store::skill_or_404(conn, id)?;
                let SkillUpdateState::Available { content_hash, .. } = &skill.update else {
                    return Ok(None);
                };
                let Some(row) = skill_store::skill_version(conn, content_hash)? else {
                    return Ok(None);
                };
                let hooks =
                    meta_get(conn, &updates::hooks_meta_key(content_hash.as_str()))?.is_some();
                Ok(Some((row, hooks)))
            })
            .await?;
        let Some((row, hooks_removed)) = found else {
            return Ok(None);
        };
        let Some(manifest) = row.files.iter().find(|f| f.path == SKILL_MANIFEST_FILE) else {
            return Ok(None);
        };
        let bytes = self.read_blob_bytes(&manifest.sha256).await?;
        let (_, description, license) = front_matter(&String::from_utf8_lossy(&bytes));
        Ok(Some(PendingFacts {
            content_hash: row.content_hash,
            description,
            license,
            hooks_removed,
        }))
    }

    pub(crate) async fn skill_update(
        &self,
        _caller: &Caller,
        request: SkillUpdate,
    ) -> Result<SkillResult> {
        let skill = self
            .update_skill_fields(
                request.skill_id,
                request.enabled,
                request.use_in,
                request.accept_new_version,
            )
            .await?;
        Ok(SkillResult { skill })
    }

    /// Turns a skill on or off, changes its scope, or moves it to the new
    /// version that waits. A new version whose scripts changed turns the
    /// skill off, unless the same change turns it on.
    pub(crate) async fn update_skill_fields(
        &self,
        id: SkillId,
        enabled: Option<bool>,
        use_in: Option<UseIn>,
        accept_new_version: bool,
    ) -> Result<Skill> {
        let use_in = match use_in {
            Some(u) => Some(self.check_use_in(u).await?),
            None => None,
        };
        let facts = if accept_new_version {
            self.pending_facts(id).await?
        } else {
            None
        };
        self.store
            .write(move |tx| {
                let before = skill_store::skill_or_404(tx.db(), id)?;
                let mut skill = before.clone();
                if accept_new_version {
                    let SkillUpdateState::Available {
                        commit,
                        content_hash,
                        scripts_changed,
                        ..
                    } = skill.update.clone()
                    else {
                        return Err(HubError::conflict(format!(
                            "The skill {} has no new version to accept.",
                            skill.name
                        )));
                    };
                    let version = skill_store::skill_version(tx.db(), &content_hash)?
                        .ok_or_else(|| {
                            HubError::conflict("The files of the new version are not on the hub. Check for updates again.")
                        })?;
                    skill.content_hash = content_hash;
                    skill.commit = commit;
                    skill.scripts = version
                        .files
                        .iter()
                        .filter(|f| is_script(f))
                        .map(|f| f.path.clone())
                        .collect();
                    skill.file_count = u32::try_from(version.files.len()).unwrap_or(u32::MAX);
                    skill.size_bytes = version.files.iter().map(|f| f.size).sum();
                    if let Some(facts) = facts.filter(|f| f.content_hash == skill.content_hash) {
                        if let Some(description) = facts.description {
                            skill.description = description;
                        }
                        if facts.license.is_some() {
                            skill.license = facts.license;
                        }
                        skill.hooks_removed = facts.hooks_removed;
                    }
                    skill.update = SkillUpdateState::UpToDate;
                    if scripts_changed {
                        skill.enabled = false;
                    }
                }
                if let Some(enabled) = enabled {
                    if enabled && let Some(reason) = &skill.blocked_reason {
                        return Err(skill_blocked(format!(
                            "The skill {} is blocked: {reason}",
                            skill.name
                        )));
                    }
                    skill.enabled = enabled;
                }
                if let Some(use_in) = use_in {
                    skill.use_in = use_in;
                }
                if skill != before {
                    skill.updated_at = tx.now();
                    tx.save_skill(&skill)?;
                }
                Ok(skill)
            })
            .await
    }

    pub(crate) async fn skill_remove(
        &self,
        _caller: &Caller,
        request: SkillRemove,
    ) -> Result<Empty> {
        let id = request.skill_id;
        self.store
            .write(move |tx| {
                skill_store::skill_or_404(tx.db(), id)?;
                tx.delete_skill(id)
            })
            .await?;
        Ok(Empty {})
    }

    /// The content hash and the files of the installed or the pending
    /// version of a skill.
    async fn skill_version_files(
        &self,
        id: SkillId,
        version: SkillVersion,
    ) -> Result<(Skill, skill_store::SkillVersionRow)> {
        self.store
            .read(move |conn| {
                let skill = skill_store::skill_or_404(conn, id)?;
                let hash = match (version, &skill.update) {
                    (SkillVersion::Installed, _) => skill.content_hash.clone(),
                    (SkillVersion::Pending, SkillUpdateState::Available { content_hash, .. }) => {
                        content_hash.clone()
                    }
                    (SkillVersion::Pending, _) => {
                        return Err(HubError::not_found(format!(
                            "The skill {} has no new version.",
                            skill.name
                        )));
                    }
                };
                let row = skill_store::skill_version(conn, &hash)?.ok_or_else(|| {
                    HubError::not_found("The files of this version are not on the hub.")
                })?;
                Ok((skill, row))
            })
            .await
    }

    pub(crate) async fn skill_files(
        &self,
        _caller: &Caller,
        request: SkillFiles,
    ) -> Result<SkillFilesResult> {
        let (_, row) = self
            .skill_version_files(request.skill_id, request.version)
            .await?;
        Ok(SkillFilesResult {
            content_hash: row.content_hash,
            files: row.files,
        })
    }

    pub(crate) async fn skill_read_file(
        &self,
        _caller: &Caller,
        request: SkillReadFile,
    ) -> Result<SkillFileContent> {
        let (_, row) = self
            .skill_version_files(request.skill_id, request.version)
            .await?;
        self.read_skill_file(&row.files, &request.path).await
    }

    /// One file of a skill version. Text over the read limit is cut; a
    /// file that is not UTF-8 has no content.
    async fn read_skill_file(
        &self,
        files: &[SkillFileInfo],
        path: &str,
    ) -> Result<SkillFileContent> {
        let path = path.trim().trim_start_matches("./");
        let file = files
            .iter()
            .find(|f| f.path == path)
            .ok_or_else(|| HubError::not_found(format!("The skill has no file {path}.")))?;
        let bytes = self.read_blob_bytes(&file.sha256).await?;
        let (content, binary, truncated) = match String::from_utf8(bytes) {
            Ok(text) => {
                if text.len() > SKILL_READ_MAX_BYTES {
                    let mut end = SKILL_READ_MAX_BYTES;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    (Some(text[..end].to_owned()), false, true)
                } else {
                    (Some(text), false, false)
                }
            }
            Err(_) => (None, true, false),
        };
        Ok(SkillFileContent {
            path: file.path.clone(),
            sha256: file.sha256.clone(),
            size: file.size,
            content,
            binary,
            truncated,
        })
    }

    /// `skill_bundle.get`: the file list of a bundle that a `session.start`
    /// to this worker named.
    pub(crate) async fn skill_bundle_get(
        &self,
        worker_id: WorkerId,
        request: SkillBundleGet,
    ) -> Result<SkillBundle> {
        let hash = request.content_hash;
        self.store
            .read(move |conn| {
                let unknown =
                    || HubError::not_found("The hub did not give this worker that skill bundle.");
                if !skill_store::has_bundle_grant(conn, worker_id, &hash)? {
                    return Err(unknown());
                }
                let row = skill_store::skill_version(conn, &hash)?.ok_or_else(unknown)?;
                Ok(SkillBundle {
                    skill_id: row.skill_id,
                    name: row.name,
                    content_hash: row.content_hash,
                    files: row.files,
                })
            })
            .await
    }

    /// The skills of the session's effective set.
    async fn session_skills(&self, ctx: &ToolCtx) -> Result<Vec<Skill>> {
        let channel_id = ctx.channel.id;
        self.store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, channel_id)?;
                skill_store::effective_skills(conn, &channel)
            })
            .await
    }

    pub(crate) async fn tool_list_skills(&self, ctx: &ToolCtx) -> Result<Value> {
        let skills = self.session_skills(ctx).await?;
        Ok(serde_json::to_value(ListSkillsResult {
            skills: skills.iter().map(SkillContextEntry::from).collect(),
        })?)
    }

    pub(crate) async fn tool_read_skill(
        &self,
        ctx: &ToolCtx,
        args: ReadSkillArgs,
    ) -> Result<Value> {
        let skills = self.session_skills(ctx).await?;
        let wanted = args.skill.trim();
        let skill = skills
            .iter()
            .find(|s| s.name == wanted || s.id.to_string() == wanted)
            .ok_or_else(|| {
                let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
                HubError::not_found(if names.is_empty() {
                    format!(
                        "There is no skill {wanted} in this channel. This channel has no skills."
                    )
                } else {
                    format!(
                        "There is no skill {wanted} in this channel. Use one of these: {}.",
                        names.join(", ")
                    )
                })
            })?;
        let (_, row) = self
            .skill_version_files(skill.id, SkillVersion::Installed)
            .await?;
        let path = args
            .path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or(SKILL_MANIFEST_FILE)
            .to_owned();
        let file = self.read_skill_file(&row.files, &path).await?;
        let content = match (file.content, file.binary) {
            (Some(text), _) => text,
            (None, _) => format!(
                "{} is a binary file of {} bytes. It cannot be shown as text.",
                file.path, file.size
            ),
        };
        let files = if file.path == SKILL_MANIFEST_FILE {
            row.files
                .iter()
                .map(|f| f.path.clone())
                .filter(|p| p != SKILL_MANIFEST_FILE)
                .collect()
        } else {
            Vec::new()
        };
        Ok(serde_json::to_value(ReadSkillResult {
            skill: skill.name.clone(),
            path: file.path,
            content,
            truncated: file.truncated,
            files,
        })?)
    }
}

/// A skill for tests: `SKILL.md` and, when asked, a script.
#[cfg(test)]
pub(crate) fn test_skill(name: &str, with_script: bool) -> NewSkill {
    let mut files = vec![NewSkillFile {
        path: SKILL_MANIFEST_FILE.to_owned(),
        bytes: format!(
            "---\nname: {name}\ndescription: >\n  Does {name} things.\n  Use it for tests.\nlicense: MIT\n---\n\n# {name}\n\nFollow these steps.\n"
        )
        .into_bytes(),
        executable: false,
    }];
    files.push(NewSkillFile {
        path: "reference/notes.md".to_owned(),
        bytes: b"Some notes.".to_vec(),
        executable: false,
    });
    if with_script {
        files.push(NewSkillFile {
            path: "scripts/run.sh".to_owned(),
            bytes: b"#!/bin/sh\necho hi\n".to_vec(),
            executable: true,
        });
    }
    NewSkill {
        source: SkillSource::Upload {
            file_name: format!("{name}.zip"),
        },
        commit: None,
        files,
        use_in: UseIn::EveryChannel,
        enabled: None,
        license: None,
        audits: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::test_hub;
    use consort_proto::model::{AuditStatus, skill_content_hash};

    #[test]
    fn front_matter_reads_plain_quoted_and_folded_values() {
        let (name, description, license) = front_matter(
            "---\nname: pdf-tools\ndescription: >\n  Reads PDFs.\n  Fills forms.\nlicense: \"Apache-2.0\"\n---\nBody",
        );
        assert_eq!(name.as_deref(), Some("pdf-tools"));
        assert_eq!(description.as_deref(), Some("Reads PDFs. Fills forms."));
        assert_eq!(license.as_deref(), Some("Apache-2.0"));
        assert_eq!(front_matter("no front matter"), (None, None, None));
    }

    #[test]
    fn only_some_harnesses_load_skills() {
        assert!(harness_loads_skills("claude"));
        assert!(harness_loads_skills("codex"));
        assert!(!harness_loads_skills("acp"));
        assert!(!harness_loads_skills("fake"));
    }

    #[tokio::test]
    async fn a_stored_skill_has_its_facts_and_blobs() {
        let (hub, _dir) = test_hub().await;
        let plain = hub.store_skill(test_skill("notes", false)).await.unwrap();
        assert!(plain.enabled);
        assert_eq!(plain.license.as_deref(), Some("MIT"));
        assert_eq!(plain.file_count, 2);
        let scripted = hub.store_skill(test_skill("runner", true)).await.unwrap();
        assert!(!scripted.enabled, "a skill with scripts starts off");
        assert_eq!(scripted.scripts, vec!["scripts/run.sh".to_owned()]);
        let files = hub
            .skill_files(
                &Caller::Hub,
                SkillFiles {
                    skill_id: scripted.id,
                    version: SkillVersion::Installed,
                },
            )
            .await
            .unwrap();
        assert_eq!(files.content_hash, scripted.content_hash);
        assert_eq!(skill_content_hash(&files.files), scripted.content_hash);
        let manifest = hub
            .skill_read_file(
                &Caller::Hub,
                SkillReadFile {
                    skill_id: scripted.id,
                    path: "SKILL.md".to_owned(),
                    version: SkillVersion::Installed,
                },
            )
            .await
            .unwrap();
        assert!(manifest.content.unwrap().contains("# runner"));
        let err = hub
            .store_skill(test_skill("notes", false))
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::Conflict);
    }

    #[tokio::test]
    async fn a_skill_with_a_failed_audit_is_blocked() {
        let (hub, _dir) = test_hub().await;
        let mut new = test_skill("risky", false);
        new.audits.push(SkillAudit {
            source: "Snyk".to_owned(),
            status: AuditStatus::Failed,
            summary: None,
            url: None,
        });
        let skill = hub.store_skill(new).await.unwrap();
        assert!(!skill.enabled);
        assert!(skill.blocked_reason.is_some());
        let err = hub
            .update_skill_fields(skill.id, Some(true), None, false)
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::SkillBlocked);
    }

    #[tokio::test]
    async fn a_skill_with_bad_files_is_refused() {
        let (hub, _dir) = test_hub().await;
        let mut new = test_skill("bad", false);
        new.files.push(NewSkillFile {
            path: "../escape".to_owned(),
            bytes: Vec::new(),
            executable: false,
        });
        let err = hub.store_skill(new).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::SkillInvalid);
        let mut new = test_skill("Bad Name", false);
        new.files.truncate(1);
        let err = hub.store_skill(new).await.unwrap_err();
        assert_eq!(err.code(), ErrorCode::SkillInvalid);
    }

    #[tokio::test]
    async fn accepting_a_new_version_with_new_scripts_turns_the_skill_off() {
        let (hub, _dir) = test_hub().await;
        let skill = hub.store_skill(test_skill("tool", true)).await.unwrap();
        let skill = hub
            .update_skill_fields(skill.id, Some(true), None, false)
            .await
            .unwrap();
        assert!(skill.enabled);
        // A new version waits: one more script.
        let mut infos = hub
            .skill_files(
                &Caller::Hub,
                SkillFiles {
                    skill_id: skill.id,
                    version: SkillVersion::Installed,
                },
            )
            .await
            .unwrap()
            .files;
        let extra = hub.put_blob_bytes(b"echo more".to_vec()).await.unwrap();
        infos.push(SkillFileInfo {
            path: "scripts/more.sh".to_owned(),
            sha256: extra,
            size: 9,
            executable: true,
        });
        let hash = skill_content_hash(&infos);
        let (id, name, stored_hash) = (skill.id, skill.name.clone(), hash.clone());
        hub.store
            .write(move |tx| {
                tx.save_skill_version(id, &name, &stored_hash, &infos)?;
                let mut skill = skill_store::skill_or_404(tx.db(), id)?;
                skill.update = SkillUpdateState::Available {
                    commit: Some("abc".to_owned()),
                    content_hash: stored_hash,
                    changes: Vec::new(),
                    scripts_changed: true,
                    found_at: tx.now(),
                };
                tx.save_skill(&skill)
            })
            .await
            .unwrap();
        let pending = hub
            .skill_files(
                &Caller::Hub,
                SkillFiles {
                    skill_id: skill.id,
                    version: SkillVersion::Pending,
                },
            )
            .await
            .unwrap();
        assert_eq!(pending.content_hash, hash);
        let moved = hub
            .update_skill_fields(skill.id, None, None, true)
            .await
            .unwrap();
        assert_eq!(moved.content_hash, hash);
        assert_eq!(moved.commit.as_deref(), Some("abc"));
        assert!(!moved.enabled);
        assert_eq!(moved.scripts.len(), 2);
        assert_eq!(moved.update, SkillUpdateState::UpToDate);
        let err = hub
            .update_skill_fields(skill.id, None, None, true)
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::Conflict);
    }
}
