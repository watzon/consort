//! The connectors and skills of a session: its effective set, as
//! `SessionStart.extensions`.

use consort_proto::ids::{ChannelId, WorkerId};
use consort_proto::model::{ConnectorContextEntry, Skill, SkillContextEntry};
use consort_proto::worker_link::SessionExtensions;
use rusqlite::Connection as Db;

use crate::error::Result;
use crate::store::{Tx, connectors as con_store, conversation, skills as skill_store};

/// The effective set of a channel, as the worker gets it.
pub(crate) fn extensions_for(conn: &Db, channel_id: ChannelId) -> Result<SessionExtensions> {
    let channel = conversation::channel_or_404(conn, channel_id)?;
    let connections = con_store::effective_connections(conn, &channel)?;
    let skills = skill_store::effective_skills(conn, &channel)?;
    Ok(SessionExtensions {
        connectors: connections
            .iter()
            .map(ConnectorContextEntry::from)
            .collect(),
        skills: skills.iter().map(Skill::bundle_ref).collect(),
        skill_context: skills.iter().map(SkillContextEntry::from).collect(),
    })
}

/// A text that changes when the effective set of a channel changes. The
/// coordinator puts it in its prompt fingerprint, so an idle coordinator
/// session is replaced when its connectors or skills change.
pub(crate) fn effective_set_key(conn: &Db, channel_id: ChannelId) -> Result<String> {
    Ok(serde_json::to_string(&extensions_for(conn, channel_id)?)?)
}

impl Tx<'_> {
    /// The extensions for a new session on a worker. The worker may then
    /// read each named bundle with `skill_bundle.get`.
    pub fn session_extensions(
        &mut self,
        worker_id: WorkerId,
        channel_id: ChannelId,
    ) -> Result<SessionExtensions> {
        let extensions = extensions_for(self.db(), channel_id)?;
        for bundle in &extensions.skills {
            self.grant_bundle(worker_id, &bundle.content_hash)?;
        }
        Ok(extensions)
    }
}
