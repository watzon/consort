//! Installed skills, the file list of each skill version, and the bundles
//! that the hub named to each worker.
//!
//! The files themselves are blobs, named by their SHA-256. A version row
//! stays after its skill moves to a new version or goes away, because a
//! session that started with it may still need it.

use consort_proto::client_api::{EntityKind, EntityPayload};
use consort_proto::ids::{SkillId, WorkerId};
use consort_proto::model::{Channel, Sha256Hash, Skill, SkillFileInfo};
use rusqlite::{Connection as Db, OptionalExtension, params};

use super::{Tx, get_doc, list_docs, parse_doc, to_doc};
use crate::error::{HubError, Result};

/// The file list of one skill version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillVersionRow {
    pub skill_id: SkillId,
    pub name: String,
    pub content_hash: Sha256Hash,
    pub files: Vec<SkillFileInfo>,
}

pub fn skill(conn: &Db, id: SkillId) -> Result<Option<Skill>> {
    get_doc(
        conn,
        "SELECT data FROM skills WHERE id = ?1",
        [id.to_string()],
    )
}

pub fn skill_or_404(conn: &Db, id: SkillId) -> Result<Skill> {
    skill(conn, id)?.ok_or_else(|| HubError::not_found("The skill does not exist."))
}

/// Every installed skill, by name.
pub fn skills(conn: &Db) -> Result<Vec<Skill>> {
    list_docs(conn, "SELECT data FROM skills ORDER BY name", [])
}

/// Finds a skill by its name or its ID.
pub fn find_skill(conn: &Db, key: &str) -> Result<Option<Skill>> {
    let key = key.trim();
    if let Ok(id) = key.parse::<SkillId>() {
        return skill(conn, id);
    }
    get_doc(conn, "SELECT data FROM skills WHERE name = ?1", [key])
}

/// The skills of a channel's effective set: on, not blocked, their
/// `use_in` matches the channel, and the channel did not turn them off.
pub fn effective_skills(conn: &Db, channel: &Channel) -> Result<Vec<Skill>> {
    Ok(skills(conn)?
        .into_iter()
        .filter(|s| s.is_available_in(channel))
        .collect())
}

pub fn skill_version(conn: &Db, content_hash: &Sha256Hash) -> Result<Option<SkillVersionRow>> {
    let row = conn
        .query_row(
            "SELECT skill_id, name, files FROM skill_versions WHERE content_hash = ?1",
            [content_hash.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    row.map(|(skill_id, name, files)| {
        Ok(SkillVersionRow {
            skill_id: skill_id
                .parse()
                .map_err(|_| HubError::internal("a stored skill ID does not parse"))?,
            name,
            content_hash: content_hash.clone(),
            files: parse_doc(&files)?,
        })
    })
    .transpose()
}

/// Whether a `session.start` to this worker named this bundle.
pub fn has_bundle_grant(conn: &Db, worker_id: WorkerId, content_hash: &Sha256Hash) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM skill_bundle_grants WHERE worker_id = ?1 AND content_hash = ?2",
            params![worker_id.to_string(), content_hash.as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

impl Tx<'_> {
    /// Stores a skill and adds a feed upsert.
    pub fn save_skill(&mut self, skill: &Skill) -> Result<()> {
        self.db().execute(
            "INSERT INTO skills (id, name, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, data = excluded.data",
            params![skill.id.to_string(), skill.name, to_doc(skill)?],
        )?;
        self.emit(EntityPayload::Skill(skill.clone()))
    }

    /// Stores the file list of a skill version. A version that is there
    /// already keeps its row: the same hash means the same files.
    pub fn save_skill_version(
        &mut self,
        skill_id: SkillId,
        name: &str,
        content_hash: &Sha256Hash,
        files: &[SkillFileInfo],
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO skill_versions (content_hash, skill_id, name, files, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(content_hash) DO NOTHING",
            params![
                content_hash.as_str(),
                skill_id.to_string(),
                name,
                to_doc(&files)?,
                self.now().as_millis()
            ],
        )?;
        Ok(())
    }

    /// Deletes a skill and its channel exceptions, and adds the feed
    /// delete. Its versions stay for sessions that use them.
    pub fn delete_skill(&mut self, id: SkillId) -> Result<()> {
        self.db()
            .execute("DELETE FROM skills WHERE id = ?1", [id.to_string()])?;
        for mut channel in super::conversation::channels(self.db())? {
            if channel.disabled_skills.contains(&id) {
                channel.disabled_skills.retain(|s| *s != id);
                channel.updated_at = self.now();
                self.save_channel(&channel)?;
            }
        }
        self.emit_delete(EntityKind::Skill, id.to_string(), None)
    }

    /// Lets a worker read a bundle with `skill_bundle.get`.
    pub fn grant_bundle(&mut self, worker_id: WorkerId, content_hash: &Sha256Hash) -> Result<()> {
        self.db().execute(
            "INSERT INTO skill_bundle_grants (worker_id, content_hash, granted_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT DO UPDATE SET granted_at = excluded.granted_at",
            params![
                worker_id.to_string(),
                content_hash.as_str(),
                self.now().as_millis()
            ],
        )?;
        Ok(())
    }
}
