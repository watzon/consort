//! Hub entities for the tests of the cache and the reducers.

use consort_proto::{
    ids::{ConnectionId, SkillId},
    model::{
        Connection, ConnectionStatus, Skill, SkillSource, SkillUpdateState, ToolApprovals, UseIn,
    },
    time::Timestamp,
};

pub(crate) fn connection(name: &str, use_in: UseIn, at: i64) -> Connection {
    Connection {
        id: ConnectionId::new(),
        name: name.into(),
        handle: consort_proto::model::connection_handle(name),
        service: "gmail".into(),
        service_name: "Gmail".into(),
        logo_url: Some("https://logos.example/gmail.svg".into()),
        identity: Some("chris@work.example".into()),
        use_in,
        approvals: ToolApprovals::default(),
        status: ConnectionStatus::Active,
        last_used_at: None,
        created_at: Timestamp::from_millis(at),
        updated_at: Timestamp::from_millis(at),
    }
}

pub(crate) fn skill(name: &str, at: i64) -> Skill {
    Skill {
        id: SkillId::new(),
        name: name.into(),
        description: "Fill and read PDF forms.".into(),
        source: SkillSource::Git {
            url: "https://github.com/anthropics/skills".into(),
            path: Some("document-skills/pdf".into()),
            git_ref: None,
        },
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        content_hash: "a".repeat(64).parse().expect("a hash"),
        license: Some("Apache-2.0".into()),
        scripts: Vec::new(),
        audits: Vec::new(),
        enabled: true,
        use_in: UseIn::EveryChannel,
        update: SkillUpdateState::UpToDate,
        blocked_reason: None,
        hooks_removed: false,
        file_count: 2,
        size_bytes: 20,
        installed_at: Timestamp::from_millis(at),
        updated_at: Timestamp::from_millis(at),
    }
}
