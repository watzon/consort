//! The daily check of installed skills for a new version.
//!
//! For each skill from a git repo or a directory, the hub asks the repo for
//! the commit of the followed branch (`git ls-remote`). A new commit whose
//! skill folder changed becomes [`SkillUpdateState::Available`] with the
//! file changes and `scripts_changed`. Its files are stored as blobs and as
//! a version row, so the user can read them before accepting. Only
//! `skill.update { accept_new_version }` moves the pin. Running sessions
//! keep the version that they started with.

use std::collections::BTreeMap;

use consort_proto::ids::SkillId;
use consort_proto::model::{
    SKILL_UPDATE_CHECK_MS, Skill, SkillFileChange, SkillFileChangeKind, SkillFileInfo, SkillSource,
    SkillUpdateState,
};
use consort_proto::time::Timestamp;

use super::package::{is_script, prepare};
use crate::error::Result;
use crate::hub::Hub;
use crate::store::{meta_get, skills as skill_store};

/// When the last full check ran, in milliseconds, in `meta`.
const CHECKED_AT_META_KEY: &str = "skill_updates_checked_at";
/// The `meta` key that marks a stored version whose `hooks` the hub removed.
pub(crate) fn hooks_meta_key(content_hash: &str) -> String {
    format!("skill_hooks_removed:{content_hash}")
}

/// What one check found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpdateReport {
    /// Skills whose source the hub asked.
    pub checked: usize,
    /// Skills with a new version to review after the check.
    pub available: usize,
    /// Skills whose check failed.
    pub failed: usize,
}

/// The file changes from `old` to `new`, sorted by path, and whether a new
/// or changed file is a script.
pub(crate) fn file_changes(
    old: &[SkillFileInfo],
    new: &[SkillFileInfo],
) -> (Vec<SkillFileChange>, bool) {
    let old: BTreeMap<&str, &SkillFileInfo> = old.iter().map(|f| (f.path.as_str(), f)).collect();
    let new_map: BTreeMap<&str, &SkillFileInfo> =
        new.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut changes = Vec::new();
    let mut scripts_changed = false;
    for (path, file) in &new_map {
        let change = match old.get(path) {
            None => Some(SkillFileChangeKind::Added),
            Some(before)
                if before.sha256 != file.sha256 || before.executable != file.executable =>
            {
                Some(SkillFileChangeKind::Modified)
            }
            Some(_) => None,
        };
        if let Some(change) = change {
            scripts_changed |= is_script(file);
            changes.push(SkillFileChange {
                path: (*path).to_owned(),
                change,
            });
        }
    }
    for path in old.keys() {
        if !new_map.contains_key(path) {
            changes.push(SkillFileChange {
                path: (*path).to_owned(),
                change: SkillFileChangeKind::Removed,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    (changes, scripts_changed)
}

impl Hub {
    /// Runs the check when the last one is older than one day.
    pub(crate) async fn check_skill_updates_if_due(&self) -> Result<Option<UpdateReport>> {
        let last = self
            .store
            .read(|conn| meta_get(conn, CHECKED_AT_META_KEY))
            .await?
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        if Timestamp::now().as_millis() - last < SKILL_UPDATE_CHECK_MS {
            return Ok(None);
        }
        self.check_skill_updates().await.map(Some)
    }

    /// Checks every skill that can update, now.
    pub(crate) async fn check_skill_updates(&self) -> Result<UpdateReport> {
        let _running = self.skill_sources.update_check.lock().await;
        let skills = self.store.read(skill_store::skills).await?;
        let mut report = UpdateReport::default();
        for skill in skills.iter().filter(|s| s.source.can_update()) {
            report.checked += 1;
            let state = match self.check_one(skill).await {
                Ok(state) => state,
                Err(err) => Some(SkillUpdateState::CheckFailed {
                    message: err.to_error_body().message,
                }),
            };
            match state.as_ref().unwrap_or(&skill.update) {
                SkillUpdateState::Available { .. } => report.available += 1,
                SkillUpdateState::CheckFailed { .. } => report.failed += 1,
                SkillUpdateState::UpToDate => {}
            }
            if let Some(state) = state {
                self.set_update_state(skill.id, &skill.content_hash, state)
                    .await?;
            }
        }
        let now = Timestamp::now().as_millis().to_string();
        self.store
            .write(move |tx| tx.meta_set(CHECKED_AT_META_KEY, &now))
            .await?;
        Ok(report)
    }

    /// Checks one skill. Returns the new update state, or `None` when it
    /// stays as it is.
    async fn check_one(&self, skill: &Skill) -> Result<Option<SkillUpdateState>> {
        // A git skill follows its ref. A directory skill follows the ref in
        // its location, or the default branch. Archive and npm skills have
        // their own pins (see `remote.rs`). The user installed a `Git`
        // skill from the URL, so git may use credentials for it.
        let (location, path, git_ref, typed) = match &skill.source {
            SkillSource::Git { url, path, git_ref } => (url, path, git_ref.as_deref(), true),
            SkillSource::Directory { url, path, .. } => (url, path, None, false),
            SkillSource::Upload { .. } => return Ok(None),
        };
        let sources = &self.skill_sources;
        let up_to_date = || {
            Ok(match skill.update {
                SkillUpdateState::CheckFailed { .. } => Some(SkillUpdateState::UpToDate),
                _ => None,
            })
        };
        let commit = self.remote_pin(location, git_ref, typed).await?;
        if skill.commit.as_deref() == Some(commit.as_str()) {
            return up_to_date();
        }
        if let SkillUpdateState::Available {
            commit: Some(waiting),
            ..
        } = &skill.update
            && *waiting == commit
        {
            return Ok(None);
        }
        let same = sources
            .same_files
            .lock()
            .ok()
            .and_then(|m| m.get(&skill.id).cloned());
        if same.as_deref() == Some(commit.as_str()) {
            return up_to_date();
        }
        let got = self
            .fetch_remote(location, path.as_deref(), git_ref, typed)
            .await?;
        let prepared = prepare(got.files, None)?;
        if prepared.name != skill.name {
            return Ok(Some(SkillUpdateState::CheckFailed {
                message: format!(
                    "The new version is named {}, not {}. Remove the skill and add it again to take it.",
                    prepared.name, skill.name
                ),
            }));
        }
        if prepared.content_hash == skill.content_hash {
            if let Ok(mut same) = sources.same_files.lock() {
                same.insert(skill.id, got.commit.clone());
            }
            return up_to_date();
        }
        let installed = {
            let hash = skill.content_hash.clone();
            self.store
                .read(move |conn| skill_store::skill_version(conn, &hash))
                .await?
                .map(|row| row.files)
                .unwrap_or_default()
        };
        let (changes, scripts_changed) = file_changes(&installed, &prepared.infos);
        for file in prepared.files {
            self.put_blob_bytes(file.bytes).await?;
        }
        let (id, name, hash, infos, hooks) = (
            skill.id,
            skill.name.clone(),
            prepared.content_hash.clone(),
            prepared.infos,
            prepared.hooks_removed,
        );
        self.store
            .write(move |tx| {
                tx.save_skill_version(id, &name, &hash, &infos)?;
                if hooks {
                    tx.meta_set(&hooks_meta_key(hash.as_str()), "1")?;
                }
                Ok(())
            })
            .await?;
        Ok(Some(SkillUpdateState::Available {
            commit: Some(got.commit.clone()),
            content_hash: prepared.content_hash,
            changes,
            scripts_changed,
            found_at: Timestamp::now(),
        }))
    }

    /// Saves the update state of a skill, unless the skill moved to another
    /// version or went away during the check.
    async fn set_update_state(
        &self,
        id: SkillId,
        checked_hash: &consort_proto::model::Sha256Hash,
        state: SkillUpdateState,
    ) -> Result<()> {
        let checked_hash = checked_hash.clone();
        self.store
            .write(move |tx| {
                let Some(mut skill) = skill_store::skill(tx.db(), id)? else {
                    return Ok(());
                };
                if skill.content_hash != checked_hash || skill.update == state {
                    return Ok(());
                }
                skill.update = state;
                skill.updated_at = tx.now();
                tx.save_skill(&skill)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(path: &str, fill: char, executable: bool) -> SkillFileInfo {
        SkillFileInfo {
            path: path.to_owned(),
            sha256: fill.to_string().repeat(64).parse().unwrap(),
            size: 1,
            executable,
        }
    }

    #[test]
    fn changes_name_each_file_and_see_scripts() {
        let old = vec![
            info("SKILL.md", 'a', false),
            info("notes.md", 'b', false),
            info("gone.md", 'c', false),
        ];
        let new = vec![
            info("SKILL.md", 'd', false),
            info("notes.md", 'b', false),
            info("extra.md", 'e', false),
        ];
        let (changes, scripts) = file_changes(&old, &new);
        let kinds: Vec<(&str, SkillFileChangeKind)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.change))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("SKILL.md", SkillFileChangeKind::Modified),
                ("extra.md", SkillFileChangeKind::Added),
                ("gone.md", SkillFileChangeKind::Removed),
            ]
        );
        assert!(!scripts);
        // A new script, or a file that becomes executable, changes scripts.
        let (_, scripts) = file_changes(&old, &[info("scripts/run.py", 'f', false)]);
        assert!(scripts);
        let (_, scripts) = file_changes(&old, &[info("notes.md", 'b', true)]);
        assert!(scripts);
    }
}
